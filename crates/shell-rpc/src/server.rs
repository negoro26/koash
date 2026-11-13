use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use shell_builtins::{BUILTIN_SCHEMAS, execute_program_value, post_edit_diagnostics};
use shell_kernel::{CowWorkspace, Session, expand_word};
use shell_syntax::{Command, Program, RedirectKind};
use shell_value::{
    StructuredDiagnostic, TaintLevel, Value, records_to_compact_sigil, redact_secrets,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use crate::protocol::{
    ExecParams, ForkParams, PolicyConfig, RestoreParams, RpcEvent, RpcRequest, RpcResponse,
    SessionIdParams, StreamRule,
};

pub struct RpcServer {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    next_id: Arc<Mutex<u64>>,
}

impl Default for RpcServer {
    fn default() -> Self {
        Self::new()
    }
}

impl RpcServer {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    pub async fn serve_stdio(&self) -> Result<(), StructuredDiagnostic> {
        self.spawn_reaper();
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        self.serve_stream(stdin, stdout).await
    }

    pub async fn serve_socket(&self, socket_path: &Path) -> Result<(), StructuredDiagnostic> {
        self.spawn_reaper();
        if socket_path.exists() {
            let _ = std::fs::remove_file(socket_path);
        }
        let listener = UnixListener::bind(socket_path)
            .map_err(|e| StructuredDiagnostic::from_io("E_SOCKET_BIND", &e, None))?;

        loop {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|e| StructuredDiagnostic::from_io("E_SOCKET_ACCEPT", &e, None))?;
            let server = Self {
                sessions: Arc::clone(&self.sessions),
                next_id: Arc::clone(&self.next_id),
            };
            tokio::spawn(async move {
                let (reader, writer) = stream.into_split();
                let _ = server.serve_stream(reader, writer).await;
            });
        }
    }

    pub async fn serve_stream<R, W>(&self, reader: R, mut writer: W) -> Result<(), StructuredDiagnostic>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| StructuredDiagnostic::from_io("E_RPC_READ", &e, None))?
        {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let req: RpcRequest = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let resp = RpcResponse {
                        id: None,
                        result: None,
                        error: Some(StructuredDiagnostic::new(
                            "E_RPC_PARSE",
                            e.to_string(),
                            None,
                            None,
                        )),
                    };
                    send_json(&mut writer, &resp).await?;
                    continue;
                }
            };

            self.handle_request(req, &mut writer).await?;
        }
        Ok(())
    }

    async fn handle_request<W: AsyncWrite + Unpin>(
        &self,
        req: RpcRequest,
        writer: &mut W,
    ) -> Result<(), StructuredDiagnostic> {
        match req.method.as_str() {
            "session.create" => {
                let sid = self.alloc_session_id().await;
                let session = Session::new(false)?;
                self.sessions.lock().await.insert(sid.clone(), session);
                let resp = RpcResponse {
                    id: req.id,
                    result: Some(serde_json::json!({ "session_id": sid })),
                    error: None,
                };
                send_json(writer, &resp).await
            }
            "session.fork" => {
                let params: ForkParams = serde_json::from_value(req.params).map_err(|e| {
                    StructuredDiagnostic::new("E_RPC_PARAMS", e.to_string(), None, None)
                })?;
                let new_sid = self.alloc_session_id().await;
                let forked = {
                    let guard = self.sessions.lock().await;
                    guard.get(&params.session_id).map(|s| {
                        if params.isolated {
                            s.fork_isolated(Some(params.session_id.clone()), &new_sid)
                        } else {
                            Ok(s.fork_detached())
                        }
                    })
                };
                match forked {
                    Some(Ok(new_sess)) => {
                        self.sessions.lock().await.insert(new_sid.clone(), new_sess);
                        let resp = RpcResponse {
                            id: req.id,
                            result: Some(serde_json::json!({ "session_id": new_sid })),
                            error: None,
                        };
                        send_json(writer, &resp).await
                    }
                    Some(Err(diag)) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(diag),
                        };
                        send_json(writer, &resp).await
                    }
                    None => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(StructuredDiagnostic::new(
                                "E_SESSION_NOT_FOUND",
                                format!("session `{}` not found", params.session_id),
                                None,
                                None,
                            )),
                        };
                        send_json(writer, &resp).await
                    }
                }
            }
            "session.diff" => {
                let params: SessionIdParams = serde_json::from_value(req.params).map_err(|e| {
                    StructuredDiagnostic::new("E_RPC_PARAMS", e.to_string(), None, None)
                })?;
                let diff = {
                    let guard = self.sessions.lock().await;
                    guard.get(&params.session_id).map(|s| {
                        s.cow
                            .as_ref()
                            .map(|c| {
                                c.diff()
                                    .into_iter()
                                    .map(|(path, state)| serde_json::json!({ "path": path, "state": state }))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                };
                match diff {
                    Some(entries) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: Some(serde_json::json!({ "changes": entries })),
                            error: None,
                        };
                        send_json(writer, &resp).await
                    }
                    None => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(StructuredDiagnostic::new(
                                "E_SESSION_NOT_FOUND",
                                format!("session `{}` not found", params.session_id),
                                None,
                                None,
                            )),
                        };
                        send_json(writer, &resp).await
                    }
                }
            }
            "session.merge" => {
                let params: SessionIdParams = serde_json::from_value(req.params).map_err(|e| {
                    StructuredDiagnostic::new("E_RPC_PARAMS", e.to_string(), None, None)
                })?;
                let merged = {
                    let mut guard = self.sessions.lock().await;
                    guard.get_mut(&params.session_id).map(|s| match s.cow.as_mut() {
                        Some(cow) => cow.merge(),
                        None => Ok(Vec::new()),
                    })
                };
                match merged {
                    Some(Ok(files)) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: Some(serde_json::json!({ "merged": files })),
                            error: None,
                        };
                        send_json(writer, &resp).await
                    }
                    Some(Err(diag)) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(diag),
                        };
                        send_json(writer, &resp).await
                    }
                    None => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(StructuredDiagnostic::new(
                                "E_SESSION_NOT_FOUND",
                                format!("session `{}` not found", params.session_id),
                                None,
                                None,
                            )),
                        };
                        send_json(writer, &resp).await
                    }
                }
            }
            "session.snapshot" => {
                let params: SessionIdParams = serde_json::from_value(req.params).map_err(|e| {
                    StructuredDiagnostic::new("E_RPC_PARAMS", e.to_string(), None, None)
                })?;
                let snap = {
                    let guard = self.sessions.lock().await;
                    guard.get(&params.session_id).map(|s| s.snapshot())
                };
                match snap {
                    Some(Ok(snapshot)) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: Some(serde_json::json!({ "snapshot": snapshot })),
                            error: None,
                        };
                        send_json(writer, &resp).await
                    }
                    Some(Err(diag)) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(diag),
                        };
                        send_json(writer, &resp).await
                    }
                    None => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(StructuredDiagnostic::new(
                                "E_SESSION_NOT_FOUND",
                                format!("session `{}` not found", params.session_id),
                                None,
                                None,
                            )),
                        };
                        send_json(writer, &resp).await
                    }
                }
            }
            "session.restore" => {
                let params: RestoreParams = serde_json::from_value(req.params).map_err(|e| {
                    StructuredDiagnostic::new("E_RPC_PARAMS", e.to_string(), None, None)
                })?;
                match Session::restore(&params.snapshot) {
                    Ok(sess) => {
                        let sid = self.alloc_session_id().await;
                        self.sessions.lock().await.insert(sid.clone(), sess);
                        let resp = RpcResponse {
                            id: req.id,
                            result: Some(serde_json::json!({ "session_id": sid })),
                            error: None,
                        };
                        send_json(writer, &resp).await
                    }
                    Err(diag) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(diag),
                        };
                        send_json(writer, &resp).await
                    }
                }
            }
            "session.schemas" => {
                let resp = RpcResponse {
                    id: req.id,
                    result: Some(serde_json::json!({ "schemas": BUILTIN_SCHEMAS })),
                    error: None,
                };
                send_json(writer, &resp).await
            }
            "exec" => {
                let params: ExecParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => {
                        let resp = RpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(StructuredDiagnostic::new(
                                "E_RPC_PARAMS",
                                e.to_string(),
                                None,
                                None,
                            )),
                        };
                        return send_json(writer, &resp).await;
                    }
                };
                self.handle_exec(req.id, params, writer).await
            }
            other => {
                let resp = RpcResponse {
                    id: req.id,
                    result: None,
                    error: Some(StructuredDiagnostic::new(
                        "E_RPC_METHOD",
                        format!("unknown method `{other}`"),
                        None,
                        None,
                    )),
                };
                send_json(writer, &resp).await
            }
        }
    }

    async fn handle_exec<W: AsyncWrite + Unpin>(
        &self,
        id: Option<serde_json::Value>,
        params: ExecParams,
        writer: &mut W,
    ) -> Result<(), StructuredDiagnostic> {
        let start = Instant::now();
        let mut guard = self.sessions.lock().await;
        let session = match guard.get_mut(&params.session_id) {
            Some(s) => {
                s.touch();
                s
            }
            None => {
                let resp = RpcResponse {
                    id,
                    result: None,
                    error: Some(StructuredDiagnostic::new(
                        "E_SESSION_NOT_FOUND",
                        format!("session `{}` not found", params.session_id),
                        None,
                        None,
                    )),
                };
                return send_json(writer, &resp).await;
            }
        };

        session.mutated_files.clear();
        session.taint = TaintLevel::TrustedLocal;

        let program = match shell_syntax::parse(&params.command) {
            Ok(p) => p,
            Err(err) => {
                let resp = RpcResponse {
                    id,
                    result: None,
                    error: Some(err.into()),
                };
                return send_json(writer, &resp).await;
            }
        };

        if let Some(reason) = check_policy(session, &program, &params.policy) {
            let blocked = RpcEvent::PolicyBlocked {
                id: id.clone(),
                reason,
                command: params.command.clone(),
            };
            send_json(writer, &blocked).await?;
            let completed = RpcEvent::Completed {
                id,
                exit_code: 1,
                duration_ms: start.elapsed().as_millis() as u64,
                mutated_files_manifest: Vec::new(),
                truncated: false,
                spill_uri: None,
                lsp_diagnostics: Vec::new(),
            };
            return send_json(writer, &completed).await;
        }

        let spawned = RpcEvent::ProcessSpawned {
            id: id.clone(),
            pid: std::process::id(),
            argv: vec![params.command.clone()],
            cwd: session.cwd.to_string_lossy().to_string(),
        };
        send_json(writer, &spawned).await?;

        let exec_fut = execute_program_value(session, &program);
        let result = if let Some(ms) = params.timeout_ms {
            match tokio::time::timeout(Duration::from_millis(ms), exec_fut).await {
                Ok(res) => res,
                Err(_) => {
                    let completed = RpcEvent::Completed {
                        id,
                        exit_code: 124,
                        duration_ms: start.elapsed().as_millis() as u64,
                        mutated_files_manifest: session.mutated_files.clone(),
                        truncated: false,
                        spill_uri: None,
                        lsp_diagnostics: Vec::new(),
                    };
                    return send_json(writer, &completed).await;
                }
            }
        } else {
            exec_fut.await
        };

        let (val, mut exit_code) = match result {
            Ok(pair) => pair,
            Err(diag) => {
                let resp = RpcResponse {
                    id,
                    result: None,
                    error: Some(diag),
                };
                return send_json(writer, &resp).await;
            }
        };

        let mut truncated = false;
        let mut spill_uri: Option<String> = None;
        let max_bytes = params.max_output_bytes.unwrap_or(usize::MAX);

        match val {
            Value::Empty => {}
            Value::Error(diag) => {
                let resp = RpcResponse {
                    id,
                    result: None,
                    error: Some(diag),
                };
                return send_json(writer, &resp).await;
            }
            Value::Batch(_) | Value::RecordStream(_) => {
                let recs = val.into_records().await?;
                let mut total_bytes = 0usize;
                let mut batch = Vec::new();
                let mut spill_buf = String::new();
                for rec in recs {
                    let serialized = serde_json::to_string(&rec).unwrap_or_default();
                    if let Some((rule, line)) = match_stream_rule(&params.stream_rules, &serialized) {
                        let ev = RpcEvent::RuleTriggered {
                            id: id.clone(),
                            rule: rule.name.clone(),
                            matched_line: line,
                        };
                        send_json(writer, &ev).await?;
                        if rule.abort_on_match {
                            exit_code = 1;
                            break;
                        }
                    }
                    if truncated {
                        spill_buf.push_str(&serialized);
                        spill_buf.push('\n');
                        continue;
                    }
                    let rec_size = serialized.len() + 1;
                    if total_bytes + rec_size > max_bytes {
                        truncated = true;
                        for existing in &batch {
                            if let Ok(s) = serde_json::to_string(existing) {
                                spill_buf.push_str(&s);
                                spill_buf.push('\n');
                            }
                        }
                        spill_buf.push_str(&serialized);
                        spill_buf.push('\n');
                        continue;
                    }
                    total_bytes += rec_size;
                    batch.push(rec);
                }
                if truncated && !spill_buf.is_empty() {
                    spill_uri = write_spill_file(spill_buf.as_bytes(), "ndjson");
                }
                if !batch.is_empty() {
                    let is_compact = params.format.as_deref() == Some("compact");
                    let chunk = if is_compact {
                        let sigil = records_to_compact_sigil(&batch);
                        let safe = if session.taint == TaintLevel::UntrustedExternal {
                            redact_secrets(&sigil, &session.env.materialize())
                        } else {
                            sigil
                        };
                        RpcEvent::StreamChunk {
                            id: id.clone(),
                            records: None,
                            compact: Some(safe),
                            stdout: None,
                        }
                    } else {
                        RpcEvent::StreamChunk {
                            id: id.clone(),
                            records: Some(batch),
                            compact: None,
                            stdout: None,
                        }
                    };
                    send_json(writer, &chunk).await?;
                }
            }
            Value::Bytes(_) | Value::RawStream(_) => {
                let bytes = val.into_raw_bytes().await?;
                let text_full = String::from_utf8_lossy(&bytes);
                let mut aborted = false;
                for line in text_full.lines() {
                    if let Some((rule, matched)) = match_stream_rule(&params.stream_rules, line) {
                        let ev = RpcEvent::RuleTriggered {
                            id: id.clone(),
                            rule: rule.name.clone(),
                            matched_line: matched,
                        };
                        send_json(writer, &ev).await?;
                        if rule.abort_on_match {
                            exit_code = 1;
                            aborted = true;
                            break;
                        }
                    }
                }
                if !aborted {
                    let slice = if bytes.len() > max_bytes {
                        truncated = true;
                        spill_uri = write_spill_file(&bytes, "log");
                        &bytes[..max_bytes]
                    } else {
                        &bytes[..]
                    };
                    if !slice.is_empty() {
                        let raw_text = String::from_utf8_lossy(slice).to_string();
                        let text = if session.taint == TaintLevel::UntrustedExternal {
                            redact_secrets(&raw_text, &session.env.materialize())
                        } else {
                            raw_text
                        };
                        let chunk = RpcEvent::StreamChunk {
                            id: id.clone(),
                            records: None,
                            compact: None,
                            stdout: Some(text),
                        };
                        send_json(writer, &chunk).await?;
                    }
                }
            }
        }

        let lsp_diagnostics = if session.mutated_files.is_empty() {
            Vec::new()
        } else {
            post_edit_diagnostics(&session.cwd, &session.mutated_files).await
        };

        let completed = RpcEvent::Completed {
            id,
            exit_code,
            duration_ms: start.elapsed().as_millis() as u64,
            mutated_files_manifest: session.mutated_files.clone(),
            truncated,
            spill_uri,
            lsp_diagnostics,
        };
        send_json(writer, &completed).await
    }

    fn spawn_reaper(&self) {
        if let Ok(cwd) = std::env::current_dir() {
            let _ = CowWorkspace::sweep_orphans(&cwd);
        }
        let _ = CowWorkspace::sweep_stale_spills(Duration::from_secs(3600));
        let sessions = Arc::clone(&self.sessions);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let mut guard = sessions.lock().await;
                let expired: Vec<String> = guard
                    .iter()
                    .filter(|(_, s)| s.is_expired())
                    .map(|(k, _)| k.clone())
                    .collect();
                for id in &expired {
                    guard.remove(id);
                }
                loop {
                    let active: std::collections::HashSet<String> = guard.keys().cloned().collect();
                    let orphans: Vec<String> = guard
                        .iter()
                        .filter(|(_, s)| {
                            s.ephemeral
                                && s.parent_id
                                    .as_ref()
                                    .is_some_and(|pid| !active.contains(pid))
                        })
                        .map(|(k, _)| k.clone())
                        .collect();
                    if orphans.is_empty() {
                        break;
                    }
                    for id in orphans {
                        guard.remove(&id);
                    }
                }
            }
        });
    }

    async fn alloc_session_id(&self) -> String {
        let mut id = self.next_id.lock().await;
        let cur = *id;
        *id += 1;
        format!("sess-{cur}")
    }
}

fn match_stream_rule<'a>(rules: &'a [StreamRule], text: &str) -> Option<(&'a StreamRule, String)> {
    for rule in rules {
        if text.contains(&rule.pattern) {
            return Some((rule, text.to_string()));
        }
    }
    None
}

fn write_spill_file(bytes: &[u8], ext: &str) -> Option<String> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("koa-spill-{}-{ts}.{ext}", std::process::id()));
    std::fs::write(&path, bytes).ok()?;
    Some(format!("spill://{}", path.display()))
}

fn check_policy(session: &Session, program: &Program, policy: &PolicyConfig) -> Option<String> {
    let network_cmds = ["curl", "wget", "nc", "ssh", "scp", "rsync"];
    let mutating_cmds = ["rm", "mv", "cp", "mkdir", "touch", "chmod", "chown", "patch"];
    let restricted = policy.dry_run || !policy.allow_network || !policy.writable_paths.is_empty();

    for item in &program.items {
        for cmd in &item.pipeline.commands {
            let simple = match cmd {
                Command::Simple(simple) => simple,
                Command::PosixFallback { .. } if restricted => {
                    return Some(
                        "opaque shell construct blocked by policy (dry_run/allow_network/writable_paths cannot be checked inside it)"
                            .to_string(),
                    );
                }
                Command::PosixFallback { .. } => continue,
            };
            let args: Vec<String> = simple
                .words
                .iter()
                .map(|w| expand_word(w, &session.env, session.last_status))
                .collect();
            let prog = args.first().map(|s| s.as_str()).unwrap_or("");

            if !policy.allow_network && network_cmds.contains(&prog) {
                return Some(format!("network command `{prog}` blocked by policy"));
            }

            let has_write_redir = simple.redirections.iter().any(|r| {
                matches!(
                    r.kind,
                    RedirectKind::StdoutTrunc
                        | RedirectKind::StdoutAppend
                        | RedirectKind::StderrTrunc
                        | RedirectKind::StderrAppend
                        | RedirectKind::StdoutAndStderrTrunc
                )
            });

            if policy.dry_run && (mutating_cmds.contains(&prog) || has_write_redir) {
                return Some(format!("mutating operation `{prog}` blocked in dry_run mode"));
            }

            if !policy.writable_paths.is_empty() {
                for redir in &simple.redirections {
                    if let Some(ref target) = redir.target
                        && matches!(
                            redir.kind,
                            RedirectKind::StdoutTrunc
                                | RedirectKind::StdoutAppend
                                | RedirectKind::StderrTrunc
                                | RedirectKind::StderrAppend
                                | RedirectKind::StdoutAndStderrTrunc
                        )
                    {
                        let p = resolve_abs(session, &expand_word(target, &session.env, session.last_status));
                        if !is_path_allowed(&p, &policy.writable_paths) {
                            return Some(format!("write to `{}` outside writable_paths", p.display()));
                        }
                    }
                }
                if mutating_cmds.contains(&prog) {
                    for arg in args.iter().skip(1) {
                        if arg.starts_with('-') {
                            continue;
                        }
                        let p = resolve_abs(session, arg);
                        if !is_path_allowed(&p, &policy.writable_paths) {
                            return Some(format!("mutation of `{}` outside writable_paths", p.display()));
                        }
                    }
                }
            }
        }
    }
    None
}

fn resolve_abs(session: &Session, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        session.cwd.join(p)
    }
}

fn is_path_allowed(target: &Path, allowed: &[String]) -> bool {
    allowed.iter().any(|prefix| target.starts_with(Path::new(prefix)))
}

async fn send_json<W: AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    msg: &T,
) -> Result<(), StructuredDiagnostic> {
    let mut line = serde_json::to_string(msg)
        .map_err(|e| StructuredDiagnostic::new("E_RPC_SER", e.to_string(), None, None))?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| StructuredDiagnostic::from_io("E_RPC_WRITE", &e, None))?;
    writer
        .flush()
        .await
        .map_err(|e| StructuredDiagnostic::from_io("E_RPC_WRITE", &e, None))?;
    Ok(())
}
