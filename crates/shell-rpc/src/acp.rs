use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shell_builtins::{Chunk, Sink, run_program};
use shell_kernel::{CowWorkspace, Session};
use shell_value::StructuredDiagnostic;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

#[derive(Debug, Deserialize)]
struct AcpRequest {
    #[serde(default)]
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AcpResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<AcpError>,
}

#[derive(Debug, Serialize)]
struct AcpError {
    code: i32,
    message: String,
}

#[derive(Debug, Serialize)]
struct AcpNotification {
    jsonrpc: &'static str,
    method: &'static str,
    params: serde_json::Value,
}

pub struct AcpServer {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    cancelled: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    next_id: Arc<Mutex<u64>>,
}

impl Default for AcpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpServer {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            cancelled: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    pub async fn serve_stdio(&self) -> Result<(), StructuredDiagnostic> {
        if let Ok(cwd) = std::env::current_dir() {
            let _ = CowWorkspace::sweep_orphans(&cwd);
        }
        let _ = CowWorkspace::sweep_stale_spills(Duration::from_secs(3600));
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        self.serve_stream(stdin, stdout).await
    }

    pub async fn serve_stream<R, W>(
        &self,
        reader: R,
        mut writer: W,
    ) -> Result<(), StructuredDiagnostic>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| StructuredDiagnostic::from_io("E_ACP_READ", &e, None))?
        {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let req: AcpRequest = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let resp = AcpResponse {
                        jsonrpc: "2.0",
                        id: None,
                        result: None,
                        error: Some(AcpError {
                            code: -32700,
                            message: e.to_string(),
                        }),
                    };
                    send_acp(&mut writer, &resp).await?;
                    continue;
                }
            };
            self.handle_request(req, &mut writer).await?;
        }
        Ok(())
    }

    async fn handle_request<W: AsyncWrite + Unpin>(
        &self,
        req: AcpRequest,
        writer: &mut W,
    ) -> Result<(), StructuredDiagnostic> {
        match req.method.as_str() {
            "initialize" => {
                let resp = AcpResponse {
                    jsonrpc: "2.0",
                    id: req.id,
                    result: Some(serde_json::json!({
                        "protocolVersion": 1,
                        "agentCapabilities": { "loadSession": false },
                        "agentInfo": { "name": "koa", "version": env!("CARGO_PKG_VERSION") }
                    })),
                    error: None,
                };
                send_acp(writer, &resp).await
            }
            "session/new" => {
                let sid = self.alloc_session_id().await;
                let sess = Session::new(false)?;
                self.sessions.lock().await.insert(sid.clone(), sess);
                self.cancelled
                    .lock()
                    .await
                    .insert(sid.clone(), Arc::new(AtomicBool::new(false)));
                let resp = AcpResponse {
                    jsonrpc: "2.0",
                    id: req.id,
                    result: Some(serde_json::json!({ "sessionId": sid })),
                    error: None,
                };
                send_acp(writer, &resp).await
            }
            "session/cancel" => {
                if let Some(sid) = req.params.get("sessionId").and_then(|v| v.as_str())
                    && let Some(flag) = self.cancelled.lock().await.get(sid)
                {
                    flag.store(true, Ordering::SeqCst);
                }
                if req.id.is_some() {
                    let resp = AcpResponse {
                        jsonrpc: "2.0",
                        id: req.id,
                        result: Some(serde_json::json!({})),
                        error: None,
                    };
                    send_acp(writer, &resp).await
                } else {
                    Ok(())
                }
            }
            "session/prompt" => {
                let Some(sid) = req
                    .params
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string)
                else {
                    return send_acp_error(writer, req.id, -32602, "missing sessionId").await;
                };
                let cmd_text = extract_prompt_text(&req.params);
                let cancel_flag = {
                    let guard = self.cancelled.lock().await;
                    guard.get(&sid).cloned()
                };
                if let Some(ref flag) = cancel_flag {
                    flag.store(false, Ordering::SeqCst);
                }
                let program = match shell_syntax::parse(&cmd_text) {
                    Ok(p) => p,
                    Err(e) => {
                        return send_acp_error(writer, req.id, -32602, &e.to_string()).await;
                    }
                };
                let Some(session) = self.sessions.lock().await.remove(&sid) else {
                    return send_acp_error(
                        writer,
                        req.id,
                        -32004,
                        &format!("session `{sid}` not found"),
                    )
                    .await;
                };
                let (tx, rx) = std::sync::mpsc::sync_channel(64);
                let executor = tokio::task::spawn_blocking(move || {
                    let mut session = session;
                    session.touch();
                    session.kill.reset();
                    session.should_exit = None;
                    let sink = Sink::Capture(tx);
                    run_program(&mut session, &program, &sink);
                    session
                });
                let collector = tokio::task::spawn_blocking(move || rx.iter().collect::<Vec<Chunk>>());
                let (session, chunks) = match tokio::join!(executor, collector) {
                    (Ok(session), Ok(chunks)) => (session, chunks),
                    _ => {
                        return send_acp_error(writer, req.id, -32603, "executor thread failed").await;
                    }
                };
                self.sessions.lock().await.insert(sid.clone(), session);
                let is_cancelled = cancel_flag
                    .as_ref()
                    .is_some_and(|f| f.load(Ordering::SeqCst));
                let text = render_chunks(chunks);
                let notif = AcpNotification {
                    jsonrpc: "2.0",
                    method: "session/update",
                    params: serde_json::json!({
                        "sessionId": sid,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": text }
                        }
                    }),
                };
                send_acp(writer, &notif).await?;
                let stop_reason = if is_cancelled { "cancelled" } else { "end_turn" };
                let resp = AcpResponse {
                    jsonrpc: "2.0",
                    id: req.id,
                    result: Some(serde_json::json!({ "stopReason": stop_reason })),
                    error: None,
                };
                send_acp(writer, &resp).await
            }
            other => send_acp_error(writer, req.id, -32601, &format!("method not found: {other}")).await,
        }
    }

    async fn alloc_session_id(&self) -> String {
        let mut guard = self.next_id.lock().await;
        let cur = *guard;
        *guard += 1;
        format!("acp-{cur}")
    }
}

fn extract_prompt_text(params: &serde_json::Value) -> String {
    if let Some(s) = params.get("prompt").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(arr) = params.get("prompt").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for item in arr {
            if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                out.push(t);
            }
        }
        return out.join("\n");
    }
    String::new()
}

fn render_chunks(chunks: Vec<Chunk>) -> String {
    let mut text = String::new();
    for chunk in chunks {
        match chunk {
            Chunk::Stdout(bytes) | Chunk::Stderr(bytes) => {
                text.push_str(&String::from_utf8_lossy(&bytes));
            }
            Chunk::Batch(batch) => {
                text.push_str(&serde_json::to_string_pretty(&batch.to_maps()).unwrap_or_default());
                text.push('\n');
            }
            Chunk::Diag(diag) => {
                text.push_str(&format!("error [{}]: {}\n", diag.code, diag.message));
            }
        }
    }
    text
}

async fn send_acp<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    msg: &T,
) -> Result<(), StructuredDiagnostic> {
    let mut line = serde_json::to_string(msg)
        .map_err(|e| StructuredDiagnostic::new("E_ACP_SERDE", e.to_string(), None, None))?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| StructuredDiagnostic::from_io("E_ACP_WRITE", &e, None))?;
    writer
        .flush()
        .await
        .map_err(|e| StructuredDiagnostic::from_io("E_ACP_FLUSH", &e, None))
}

async fn send_acp_error<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: Option<serde_json::Value>,
    code: i32,
    message: &str,
) -> Result<(), StructuredDiagnostic> {
    let resp = AcpResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(AcpError {
            code,
            message: message.to_string(),
        }),
    };
    send_acp(writer, &resp).await
}
