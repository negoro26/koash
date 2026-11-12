use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustc_hash::FxHashMap;
use serde::Serialize;
use serde_json::{Value as JsonVal, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, watch};

#[derive(Debug, Clone, Serialize)]
pub struct LspDiagnostic {
    pub file: String,
    pub line: u64,
    pub severity: String,
    pub code: String,
    pub message: String,
}

pub struct WarmLspClient {
    _child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next_id: Mutex<u64>,
    versions: Mutex<FxHashMap<PathBuf, i32>>,
    diag_rx: watch::Receiver<(u64, FxHashMap<String, Vec<LspDiagnostic>>)>,
}

static LSP_POOL: OnceLock<Mutex<FxHashMap<PathBuf, Arc<WarmLspClient>>>> = OnceLock::new();

fn pool() -> &'static Mutex<FxHashMap<PathBuf, Arc<WarmLspClient>>> {
    LSP_POOL.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn detect_server(root: &Path) -> Option<(&'static str, &'static [&'static str])> {
    if root.join("Cargo.toml").is_file() {
        Some(("rust-analyzer", &[]))
    } else if root.join("package.json").is_file() {
        Some(("typescript-language-server", &["--stdio"]))
    } else if root.join("pyproject.toml").is_file() {
        Some(("pyright-langserver", &["--stdio"]))
    } else {
        None
    }
}

fn binary_in_path(bin: &str) -> bool {
    let Ok(path_var) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|d| d.join(bin).is_file())
}

pub async fn get_or_start_lsp(root: &Path) -> Option<Arc<WarmLspClient>> {
    let (bin, args) = detect_server(root)?;
    if !binary_in_path(bin) {
        return None;
    }

    let mut guard = pool().lock().await;
    if let Some(client) = guard.get(root) {
        return Some(Arc::clone(client));
    }

    let client = WarmLspClient::spawn(root, bin, args).await.ok()?;
    let arc = Arc::new(client);
    guard.insert(root.to_path_buf(), Arc::clone(&arc));
    Some(arc)
}

pub async fn post_edit_diagnostics(root: &Path, mutated_files: &[String]) -> Vec<LspDiagnostic> {
    if mutated_files.is_empty() {
        return Vec::new();
    }
    let Some(client) = get_or_start_lsp(root).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path_str in mutated_files {
        let p = Path::new(path_str);
        if !p.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(p) {
            let diags = client.did_change_and_wait(p, &content, 250).await;
            out.extend(diags);
        }
    }
    out
}

impl WarmLspClient {
    async fn spawn(root: &Path, bin: &str, args: &[&str]) -> Result<Self, std::io::Error> {
        let mut child = Command::new(bin)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("missing lsp stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("missing lsp stdout"))?;

        let (diag_tx, diag_rx) = watch::channel((0u64, FxHashMap::default()));

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut header_line = String::new();
            let mut payload = Vec::new();
            let mut rev = 0u64;
            let mut all_diags: FxHashMap<String, Vec<LspDiagnostic>> = FxHashMap::default();

            loop {
                let mut content_len: usize = 0;
                loop {
                    header_line.clear();
                    match reader.read_line(&mut header_line).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            let trimmed = header_line.trim();
                            if trimmed.is_empty() {
                                break;
                            }
                            if let Some(len_str) = trimmed.strip_prefix("Content-Length:")
                                && let Ok(n) = len_str.trim().parse::<usize>()
                            {
                                content_len = n;
                            }
                        }
                    }
                }
                if content_len == 0 {
                    continue;
                }
                payload.resize(content_len, 0);
                if reader.read_exact(&mut payload).await.is_err() {
                    return;
                }
                if let Ok(msg) = serde_json::from_slice::<JsonVal>(&payload)
                    && msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics")
                    && let Some(params) = msg.get("params")
                {
                    let uri = params
                        .get("uri")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut parsed = Vec::new();
                    if let Some(arr) = params.get("diagnostics").and_then(|d| d.as_array()) {
                        for item in arr {
                            let sev = match item.get("severity").and_then(|s| s.as_u64()).unwrap_or(1) {
                                1 => "error",
                                2 => "warning",
                                _ => "info",
                            };
                            let line = item
                                .get("range")
                                .and_then(|r| r.get("start"))
                                .and_then(|s| s.get("line"))
                                .and_then(|l| l.as_u64())
                                .unwrap_or(0)
                                + 1;
                            let code = item
                                .get("code")
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string();
                            let message = item
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("")
                                .to_string();
                            parsed.push(LspDiagnostic {
                                file: uri.trim_start_matches("file://").to_string(),
                                line,
                                severity: sev.to_string(),
                                code,
                                message,
                            });
                        }
                    }
                    all_diags.insert(uri, parsed);
                    rev += 1;
                    let _ = diag_tx.send((rev, all_diags.clone()));
                }
            }
        });

        let client = Self {
            _child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next_id: Mutex::new(1),
            versions: Mutex::new(FxHashMap::default()),
            diag_rx,
        };

        let root_uri = format!("file://{}", root.display());
        client
            .send_request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "capabilities": {}
                }),
            )
            .await?;
        client.send_notification("initialized", json!({})).await?;

        Ok(client)
    }

    async fn send_raw(&self, body: &JsonVal) -> Result<(), std::io::Error> {
        let bytes = serde_json::to_vec(body).map_err(std::io::Error::other)?;
        let header = format!("Content-Length: {}\r\n\r\n", bytes.len());
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(&bytes).await?;
        stdin.flush().await
    }

    async fn send_request(&self, method: &str, params: JsonVal) -> Result<(), std::io::Error> {
        let id = {
            let mut g = self.next_id.lock().await;
            let cur = *g;
            *g += 1;
            cur
        };
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await
    }

    async fn send_notification(&self, method: &str, params: JsonVal) -> Result<(), std::io::Error> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .await
    }

    pub async fn did_change_and_wait(
        &self,
        path: &Path,
        content: &str,
        timeout_ms: u64,
    ) -> Vec<LspDiagnostic> {
        let uri = format!("file://{}", path.display());
        let mut versions = self.versions.lock().await;
        let is_open = versions.contains_key(path);
        let ver = versions.entry(path.to_path_buf()).or_insert(0);
        *ver += 1;
        let version = *ver;
        drop(versions);

        let start_rev = self.diag_rx.borrow().0;

        if !is_open {
            let lang = match path.extension().and_then(|e| e.to_str()) {
                Some("rs") => "rust",
                Some("ts") => "typescript",
                Some("js") => "javascript",
                Some("py") => "python",
                _ => "plaintext",
            };
            let _ = self
                .send_notification(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": lang,
                            "version": version,
                            "text": content
                        }
                    }),
                )
                .await;
        } else {
            let _ = self
                .send_notification(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": version },
                        "contentChanges": [{ "text": content }]
                    }),
                )
                .await;
        }

        let mut rx = self.diag_rx.clone();
        let wait_fut = async {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                if rx.borrow().0 > start_rev {
                    break;
                }
            }
        };
        let _ = tokio::time::timeout(Duration::from_millis(timeout_ms), wait_fut).await;
        let guard = rx.borrow();
        guard.1.get(&uri).cloned().unwrap_or_default()
    }
}
