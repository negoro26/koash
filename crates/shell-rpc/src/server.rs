use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use shell_builtins::{BUILTIN_SCHEMAS, Chunk, Sink, post_edit_diagnostics, run_program};
use shell_kernel::{CowWorkspace, KillSwitch, Session, expand_word};
use shell_syntax::{Command, Program, RedirectKind};
use shell_value::{
    StructuredDiagnostic, StructuredRecord, TaintLevel, records_to_compact_sigil, redact_secrets,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use crate::protocol::{
    ExecParams, ForkParams, PolicyConfig, RestoreParams, RpcEvent, RpcRequest, RpcResponse,
    SessionIdParams, StreamRule,
};

/// How long a killed exec gets to wind down before its session is reported busy.
const KILL_GRACE: Duration = Duration::from_millis(500);
const TIMEOUT_EXIT_CODE: i32 = 124;

/// A session is out of its slot while an exec owns it.
enum Slot {
    Ready(Box<Session>),
    Busy,
}

type Sessions = Arc<Mutex<HashMap<String, Slot>>>;

fn not_found(id: &str) -> StructuredDiagnostic {
    StructuredDiagnostic::new(
        "E_SESSION_NOT_FOUND",
        format!("session `{id}` not found"),
        None,
        None,
    )
}

fn busy(id: &str) -> StructuredDiagnostic {
    StructuredDiagnostic::new(
        "E_SESSION_BUSY",
        format!("session `{id}` is still running a previous command"),
        None,
        None,
    )
}

fn ready<'a>(
    sessions: &'a HashMap<String, Slot>,
    id: &str,
) -> Result<&'a Session, StructuredDiagnostic> {
    match sessions.get(id) {
        Some(Slot::Ready(session)) => Ok(session),
        Some(Slot::Busy) => Err(busy(id)),
        None => Err(not_found(id)),
    }
}

fn ready_mut<'a>(
    sessions: &'a mut HashMap<String, Slot>,
    id: &str,
) -> Result<&'a mut Session, StructuredDiagnostic> {
    match sessions.get_mut(id) {
        Some(Slot::Ready(session)) => Ok(session),
        Some(Slot::Busy) => Err(busy(id)),
        None => Err(not_found(id)),
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
) -> Result<T, StructuredDiagnostic> {
    serde_json::from_value(params)
        .map_err(|e| StructuredDiagnostic::new("E_RPC_PARAMS", e.to_string(), None, None))
}

async fn respond<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: Option<serde_json::Value>,
    result: Result<serde_json::Value, StructuredDiagnostic>,
) -> Result<(), StructuredDiagnostic> {
    let resp = match result {
        Ok(value) => RpcResponse {
            id,
            result: Some(value),
            error: None,
        },
        Err(diag) => RpcResponse {
            id,
            result: None,
            error: Some(diag),
        },
    };
    send_json(writer, &resp).await
}

pub struct RpcServer {
    sessions: Sessions,
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
                let outcome = match Session::new(false) {
                    Ok(session) => {
                        let sid = self.alloc_session_id().await;
                        self.sessions
                            .lock()
                            .await
                            .insert(sid.clone(), Slot::Ready(Box::new(session)));
                        Ok(serde_json::json!({ "session_id": sid }))
                    }
                    Err(diag) => Err(diag),
                };
                respond(writer, req.id, outcome).await
            }
            "session.fork" => {
                let params: ForkParams = match parse_params(req.params) {
                    Ok(p) => p,
                    Err(diag) => return respond(writer, req.id, Err(diag)).await,
                };
                let new_sid = self.alloc_session_id().await;
                let forked = {
                    let guard = self.sessions.lock().await;
                    ready(&guard, &params.session_id).and_then(|s| {
                        if params.isolated {
                            s.fork_isolated(Some(params.session_id.clone()), &new_sid)
                        } else {
                            Ok(s.fork_detached())
                        }
                    })
                };
                let outcome = forked.map(|session| {
                    (new_sid.clone(), session)
                });
                match outcome {
                    Ok((sid, session)) => {
                        self.sessions.lock().await.insert(sid.clone(), Slot::Ready(Box::new(session)));
                        respond(writer, req.id, Ok(serde_json::json!({ "session_id": sid }))).await
                    }
                    Err(diag) => respond(writer, req.id, Err(diag)).await,
                }
            }
            "session.diff" => {
                let params: SessionIdParams = match parse_params(req.params) {
                    Ok(p) => p,
                    Err(diag) => return respond(writer, req.id, Err(diag)).await,
                };
                let outcome = {
                    let guard = self.sessions.lock().await;
                    ready(&guard, &params.session_id).map(|s| {
                        let entries = s
                            .cow
                            .as_ref()
                            .map(|c| {
                                c.diff()
                                    .into_iter()
                                    .map(|(path, state)| serde_json::json!({ "path": path, "state": state }))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        serde_json::json!({ "changes": entries })
                    })
                };
                respond(writer, req.id, outcome).await
            }
            "session.merge" => {
                let params: SessionIdParams = match parse_params(req.params) {
                    Ok(p) => p,
                    Err(diag) => return respond(writer, req.id, Err(diag)).await,
                };
                let outcome = {
                    let mut guard = self.sessions.lock().await;
                    ready_mut(&mut guard, &params.session_id).and_then(|s| {
                        let files = match s.cow.as_mut() {
                            Some(cow) => cow.merge()?,
                            None => Vec::new(),
                        };
                        Ok(serde_json::json!({ "merged": files }))
                    })
                };
                respond(writer, req.id, outcome).await
            }
            "session.snapshot" => {
                let params: SessionIdParams = match parse_params(req.params) {
                    Ok(p) => p,
                    Err(diag) => return respond(writer, req.id, Err(diag)).await,
                };
                let outcome = {
                    let guard = self.sessions.lock().await;
                    ready(&guard, &params.session_id)
                        .and_then(|s| s.snapshot())
                        .map(|snapshot| serde_json::json!({ "snapshot": snapshot }))
                };
                respond(writer, req.id, outcome).await
            }
            "session.restore" => {
                let params: RestoreParams = match parse_params(req.params) {
                    Ok(p) => p,
                    Err(diag) => return respond(writer, req.id, Err(diag)).await,
                };
                let outcome = match Session::restore(&params.snapshot) {
                    Ok(session) => {
                        let sid = self.alloc_session_id().await;
                        self.sessions
                            .lock()
                            .await
                            .insert(sid.clone(), Slot::Ready(Box::new(session)));
                        Ok(serde_json::json!({ "session_id": sid }))
                    }
                    Err(diag) => Err(diag),
                };
                respond(writer, req.id, outcome).await
            }
            "session.schemas" => {
                respond(
                    writer,
                    req.id,
                    Ok(serde_json::json!({ "schemas": BUILTIN_SCHEMAS })),
                )
                .await
            }
            "exec" => match parse_params::<ExecParams>(req.params) {
                Ok(params) => self.handle_exec(req.id, params, writer).await,
                Err(diag) => respond(writer, req.id, Err(diag)).await,
            },
            other => {
                let diag = StructuredDiagnostic::new(
                    "E_RPC_METHOD",
                    format!("unknown method `{other}`"),
                    None,
                    None,
                );
                respond(writer, req.id, Err(diag)).await
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
        let admission = {
            let mut guard = self.sessions.lock().await;
            admit(&self.sessions, &mut guard, &params)
        };
        let (lease, program) = match admission {
            Admission::Run(lease, program) => (lease, program),
            Admission::Reject(diag) => return respond(writer, id, Err(diag)).await,
            Admission::Blocked(reason) => {
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
        };

        let spawned = RpcEvent::ProcessSpawned {
            id: id.clone(),
            pid: std::process::id(),
            argv: vec![params.command.clone()],
            cwd: lease.cwd.to_string_lossy().to_string(),
        };
        send_json(writer, &spawned).await?;

        lease.kill.reset();
        let kill = lease.kill.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Chunk>(256);
        let mut executor = tokio::task::spawn_blocking(move || {
            lease.run_blocking(|session| {
                let sink = Sink::Capture(tx);
                let code = run_program(session, &program, &sink);
                RunSummary {
                    code,
                    redact_env: (session.taint == TaintLevel::UntrustedExternal)
                        .then(|| session.env.materialize()),
                    cwd: session.cwd.clone(),
                    mutated_files: session.mutated_files.clone(),
                }
            })
        });
        let collector = tokio::task::spawn_blocking(move || RunOutput::from_chunks(rx));

        let (finished, timed_out) = match params.timeout_ms {
            None => (Some((&mut executor).await), false),
            Some(ms) => match tokio::time::timeout(Duration::from_millis(ms), &mut executor).await {
                Ok(joined) => (Some(joined), false),
                Err(_) => {
                    kill.kill();
                    match tokio::time::timeout(KILL_GRACE, &mut executor).await {
                        Ok(joined) => (Some(joined), true),
                        Err(_) => (None, true),
                    }
                }
            },
        };

        let run = match finished {
            Some(Ok(Some(run))) => run,
            Some(Ok(None)) => {
                let diag = StructuredDiagnostic::new(
                    "E_EXEC_FAILED",
                    "executor lost its session",
                    None,
                    None,
                );
                return respond(writer, id, Err(diag)).await;
            }
            Some(Err(join_err)) => {
                let diag = StructuredDiagnostic::new(
                    "E_EXEC_FAILED",
                    format!("executor thread failed: {join_err}"),
                    None,
                    None,
                );
                return respond(writer, id, Err(diag)).await;
            }
            None => {
                // The executor still holds the session and returns it to its slot when it finishes.
                let completed = RpcEvent::Completed {
                    id,
                    exit_code: TIMEOUT_EXIT_CODE,
                    duration_ms: start.elapsed().as_millis() as u64,
                    mutated_files_manifest: Vec::new(),
                    truncated: false,
                    spill_uri: None,
                    lsp_diagnostics: Vec::new(),
                };
                return send_json(writer, &completed).await;
            }
        };

        let out = collector.await.unwrap_or_else(|_| RunOutput::empty());
        let RunSummary {
            code,
            redact_env,
            cwd,
            mutated_files,
        } = run;

        let exit_code = if timed_out { TIMEOUT_EXIT_CODE } else { code };
        let summary = send_output(writer, &id, &params, out, redact_env.as_ref(), exit_code).await?;

        let lsp_diagnostics = if mutated_files.is_empty() {
            Vec::new()
        } else {
            post_edit_diagnostics(&cwd, &mutated_files).await
        };

        let completed = RpcEvent::Completed {
            id,
            exit_code: summary.exit_code,
            duration_ms: start.elapsed().as_millis() as u64,
            mutated_files_manifest: mutated_files,
            truncated: summary.truncated,
            spill_uri: summary.spill_uri,
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
                    .filter(|(_, slot)| matches!(slot, Slot::Ready(s) if s.is_expired()))
                    .map(|(k, _)| k.clone())
                    .collect();
                for id in &expired {
                    guard.remove(id);
                }
                loop {
                    let active: std::collections::HashSet<String> = guard.keys().cloned().collect();
                    let orphans: Vec<String> = guard
                        .iter()
                        .filter(|(_, slot)| {
                            matches!(slot, Slot::Ready(s) if s.ephemeral
                                && s.parent_id.as_ref().is_some_and(|pid| !active.contains(pid)))
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

enum Admission {
    Run(Lease, Program),
    Reject(StructuredDiagnostic),
    Blocked(String),
}

/// What the caller of an exec needs from the session once the program has run.
struct RunSummary {
    code: i32,
    redact_env: Option<BTreeMap<String, String>>,
    cwd: PathBuf,
    mutated_files: Vec<String>,
}

/// Ownership of a session taken out of its slot. However the exec ends, the slot is settled:
/// dropping the lease puts the session back, or removes the slot if the executor panicked.
struct Lease {
    sessions: Sessions,
    id: String,
    runtime: tokio::runtime::Handle,
    session: Option<Box<Session>>,
    kill: KillSwitch,
    cwd: PathBuf,
}

impl Lease {
    /// Runs `f` on the session, then makes the session available again before returning.
    /// Must be called from a blocking thread.
    fn run_blocking<R>(mut self, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        let result = f(self.session.as_deref_mut()?);
        let session = self.session.take()?;
        self.sessions
            .blocking_lock()
            .insert(std::mem::take(&mut self.id), Slot::Ready(session));
        Some(result)
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let slot = (!std::thread::panicking()).then_some(Slot::Ready(session));
        let id = std::mem::take(&mut self.id);
        let settle = move |sessions: &mut HashMap<String, Slot>| match slot {
            Some(slot) => {
                sessions.insert(id, slot);
            }
            None => {
                sessions.remove(&id);
            }
        };
        match self.sessions.try_lock() {
            Ok(mut sessions) => settle(&mut sessions),
            Err(_) => {
                let sessions = Arc::clone(&self.sessions);
                self.runtime.spawn(async move { settle(&mut *sessions.lock().await) });
            }
        }
    }
}

/// Checks that `params` may run and, if so, moves the session out of its slot.
fn admit(sessions: &Sessions, slots: &mut HashMap<String, Slot>, params: &ExecParams) -> Admission {
    let id = &params.session_id;
    let program = {
        let session = match ready_mut(slots, id) {
            Ok(s) => s,
            Err(diag) => return Admission::Reject(diag),
        };
        session.touch();
        session.mutated_files.clear();
        session.taint = TaintLevel::TrustedLocal;
        session.should_exit = None;
        let program = match shell_syntax::parse(&params.command) {
            Ok(p) => p,
            Err(err) => return Admission::Reject(err.into()),
        };
        if let Some(reason) = check_policy(session, &program, &params.policy) {
            return Admission::Blocked(reason);
        }
        program
    };
    match slots.insert(id.clone(), Slot::Busy) {
        Some(Slot::Ready(session)) => Admission::Run(
            Lease {
                sessions: Arc::clone(sessions),
                id: id.clone(),
                runtime: tokio::runtime::Handle::current(),
                kill: session.kill.clone(),
                cwd: session.cwd.clone(),
                session: Some(session),
            },
            program,
        ),
        _ => Admission::Reject(busy(id)),
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

/// Everything a program wrote, gathered from the capture channel.
struct RunOutput {
    records: Vec<StructuredRecord>,
    stdout: Vec<u8>,
    stderr: String,
}

impl RunOutput {
    fn empty() -> Self {
        Self {
            records: Vec::new(),
            stdout: Vec::new(),
            stderr: String::new(),
        }
    }

    fn from_chunks(chunks: impl IntoIterator<Item = Chunk>) -> Self {
        let mut out = Self::empty();
        for chunk in chunks {
            match chunk {
                Chunk::Stdout(bytes) => out.stdout.extend_from_slice(&bytes),
                Chunk::Stderr(bytes) => out.stderr.push_str(&String::from_utf8_lossy(&bytes)),
                Chunk::Batch(batch) => out.records.extend(batch.to_maps()),
                Chunk::Diag(diag) => {
                    out.stderr.push_str(&diag.to_string());
                    out.stderr.push('\n');
                }
            }
        }
        out
    }
}

struct OutputSummary {
    exit_code: i32,
    truncated: bool,
    spill_uri: Option<String>,
}

/// Applies stream rules, the byte budget, spilling and redaction to `out` and sends it as stream chunks.
async fn send_output<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: &Option<serde_json::Value>,
    params: &ExecParams,
    out: RunOutput,
    redact_env: Option<&BTreeMap<String, String>>,
    mut exit_code: i32,
) -> Result<OutputSummary, StructuredDiagnostic> {
    let mut truncated = false;
    let mut spill_uri: Option<String> = None;
    let max_bytes = params.max_output_bytes.unwrap_or(usize::MAX);

    if !out.records.is_empty() {
        let mut total_bytes = 0usize;
        let mut batch = Vec::new();
        let mut spill_buf = String::new();
        let mut record_truncated = false;
        for rec in out.records {
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
            if record_truncated {
                spill_buf.push_str(&serialized);
                spill_buf.push('\n');
                continue;
            }
            let rec_size = serialized.len() + 1;
            if total_bytes + rec_size > max_bytes {
                record_truncated = true;
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
        if record_truncated {
            truncated = true;
            if !spill_buf.is_empty() {
                spill_uri = write_spill_file(spill_buf.as_bytes(), "ndjson");
            }
        }
        if !batch.is_empty() {
            let chunk = if params.format.as_deref() == Some("compact") {
                let sigil = records_to_compact_sigil(&batch);
                let safe = match redact_env {
                    Some(env) => redact_secrets(&sigil, env),
                    None => sigil,
                };
                RpcEvent::StreamChunk {
                    id: id.clone(),
                    records: None,
                    compact: Some(safe),
                    stdout: None,
                    stderr: None,
                }
            } else {
                RpcEvent::StreamChunk {
                    id: id.clone(),
                    records: Some(batch),
                    compact: None,
                    stdout: None,
                    stderr: None,
                }
            };
            send_json(writer, &chunk).await?;
        }
    }

    if !out.stdout.is_empty() {
        let bytes = out.stdout;
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
                spill_uri = spill_uri.or_else(|| write_spill_file(&bytes, "log"));
                &bytes[..max_bytes]
            } else {
                &bytes[..]
            };
            if !slice.is_empty() {
                let raw_text = String::from_utf8_lossy(slice).to_string();
                let text = match redact_env {
                    Some(env) => redact_secrets(&raw_text, env),
                    None => raw_text,
                };
                let chunk = RpcEvent::StreamChunk {
                    id: id.clone(),
                    records: None,
                    compact: None,
                    stdout: Some(text),
                    stderr: None,
                };
                send_json(writer, &chunk).await?;
            }
        }
    }

    if !out.stderr.is_empty() {
        let text = match redact_env {
            Some(env) => redact_secrets(&out.stderr, env),
            None => out.stderr,
        };
        let chunk = RpcEvent::StreamChunk {
            id: id.clone(),
            records: None,
            compact: None,
            stdout: None,
            stderr: Some(text),
        };
        send_json(writer, &chunk).await?;
    }

    Ok(OutputSummary {
        exit_code,
        truncated,
        spill_uri,
    })
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
