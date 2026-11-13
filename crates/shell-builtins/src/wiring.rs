use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::Scope;
use std::time::Duration;

use nix::unistd::Pid;
use shell_kernel::{
    ChildGroup, ExternalSpec, Session, SpawnError, expand_word, group_stopped, resume_group,
    spawn_external,
};
use shell_syntax::{RedirectKind, Redirection};
use shell_value::{StructuredDiagnostic, Value};

use crate::pipeline::Stage;
use crate::sink::{Chunk, Sink};

const STOP_POLL: Duration = Duration::from_millis(50);

pub(crate) enum Flow {
    None,
    Value(Value),
    Pipe(OwnedFd),
}

#[derive(Default)]
pub(crate) struct Launched {
    pub pgid: Option<Pid>,
    pub pids: Vec<Pid>,
    pub last_pid: Option<Pid>,
    pub tracked: Option<Pid>,
}

pub(crate) struct LaunchError {
    pub status: i32,
    pub diag: StructuredDiagnostic,
}

impl From<SpawnError> for LaunchError {
    fn from(err: SpawnError) -> Self {
        Self {
            status: err.status,
            diag: err.diag,
        }
    }
}

impl From<StructuredDiagnostic> for LaunchError {
    fn from(diag: StructuredDiagnostic) -> Self {
        Self { status: 1, diag }
    }
}

fn io_error(code: &str, what: &str, e: &io::Error) -> LaunchError {
    StructuredDiagnostic::new(code, format!("{what}: {e}"), None, None).into()
}

pub(crate) struct Plan<'scope, 'env> {
    pub sink: &'env Sink,
    pub scope: &'scope Scope<'scope, 'env>,
    pub background: bool,
}

enum Stream {
    Out,
    Err,
}

enum Fd {
    Inherit(Stream),
    Null,
    Owned(OwnedFd),
}

impl Fd {
    fn try_clone(&self) -> io::Result<Fd> {
        Ok(match self {
            Fd::Inherit(Stream::Out) => Fd::Owned(io::stdout().as_fd().try_clone_to_owned()?),
            Fd::Inherit(Stream::Err) => Fd::Owned(io::stderr().as_fd().try_clone_to_owned()?),
            Fd::Null => Fd::Null,
            Fd::Owned(fd) => Fd::Owned(fd.try_clone()?),
        })
    }

    fn into_stdio(self) -> Stdio {
        match self {
            Fd::Inherit(_) => Stdio::inherit(),
            Fd::Null => Stdio::null(),
            Fd::Owned(fd) => Stdio::from(fd),
        }
    }
}

enum OutTarget {
    Default,
    File(File),
}

enum ErrTarget {
    Default,
    ToOut,
    File(File),
}

pub(crate) struct Redirs {
    pub stdin: Option<File>,
    out: OutTarget,
    err: ErrTarget,
}

fn target_path(session: &Session, redir: &Redirection) -> Result<PathBuf, StructuredDiagnostic> {
    let Some(word) = &redir.target else {
        return Err(StructuredDiagnostic::new(
            "E_REDIR",
            "missing redirection target",
            Some(redir.span),
            None,
        ));
    };
    let raw = expand_word(word, &session.env, session.last_status);
    Ok(session.cwd.join(raw))
}

fn redir_error(path: &std::path::Path, redir: &Redirection, e: &io::Error) -> StructuredDiagnostic {
    StructuredDiagnostic::new(
        "E_REDIR",
        format!("{}: {e}", path.display()),
        Some(redir.span),
        None,
    )
}

fn open_for_write(
    session: &mut Session,
    redir: &Redirection,
    append: bool,
) -> Result<File, StructuredDiagnostic> {
    let path = target_path(session, redir)?;
    session.record_output_target(&path);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .mode(0o644)
        .open(&path)
        .map_err(|e| redir_error(&path, redir, &e))?;
    session.record_mutation(path.to_string_lossy().to_string());
    Ok(file)
}

pub(crate) fn read_redirect_file(
    session: &Session,
    redir: &Redirection,
) -> Result<File, StructuredDiagnostic> {
    let path = target_path(session, redir)?;
    File::open(&path).map_err(|e| redir_error(&path, redir, &e))
}

pub(crate) fn resolve_redirs(
    session: &mut Session,
    redirs: &[Redirection],
) -> Result<Redirs, StructuredDiagnostic> {
    let mut resolved = Redirs {
        stdin: None,
        out: OutTarget::Default,
        err: ErrTarget::Default,
    };
    for redir in redirs {
        match redir.kind {
            RedirectKind::Stdin => {
                resolved.stdin = Some(read_redirect_file(session, redir)?);
            }
            RedirectKind::StdoutTrunc => {
                resolved.out = OutTarget::File(open_for_write(session, redir, false)?);
            }
            RedirectKind::StdoutAppend => {
                resolved.out = OutTarget::File(open_for_write(session, redir, true)?);
            }
            RedirectKind::StderrTrunc => {
                resolved.err = ErrTarget::File(open_for_write(session, redir, false)?);
            }
            RedirectKind::StderrAppend => {
                resolved.err = ErrTarget::File(open_for_write(session, redir, true)?);
            }
            RedirectKind::StdoutAndStderrTrunc => {
                let file = open_for_write(session, redir, false)?;
                let twin = file.try_clone().map_err(|e| {
                    StructuredDiagnostic::from_io("E_REDIR", &e, Some(redir.span))
                })?;
                resolved.err = ErrTarget::File(twin);
                resolved.out = OutTarget::File(file);
            }
            RedirectKind::StderrToStdout => {
                resolved.err = match &resolved.out {
                    OutTarget::Default => ErrTarget::ToOut,
                    OutTarget::File(file) => {
                        ErrTarget::File(file.try_clone().map_err(|e| {
                            StructuredDiagnostic::from_io("E_REDIR", &e, Some(redir.span))
                        })?)
                    }
                };
            }
        }
    }
    Ok(resolved)
}

fn pipe() -> Result<(OwnedFd, OwnedFd), LaunchError> {
    io::pipe()
        .map(|(r, w)| (OwnedFd::from(r), OwnedFd::from(w)))
        .map_err(|e| io_error("E_PIPE", "pipe", &e))
}

fn spawn_thread<'scope, 'env, F>(
    scope: &'scope Scope<'scope, 'env>,
    work: F,
) -> Result<(), LaunchError>
where
    F: FnOnce() + Send + 'scope,
{
    std::thread::Builder::new()
        .spawn_scoped(scope, work)
        .map(|_| ())
        .map_err(|e| io_error("E_THREAD", "thread", &e))
}

fn feed<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    bytes: Vec<u8>,
) -> Result<Stdio, LaunchError> {
    if bytes.is_empty() {
        return Ok(Stdio::null());
    }
    let (reader, writer) = pipe()?;
    spawn_thread(scope, move || {
        let mut writer = File::from(writer);
        // A child that exits early closes the pipe; EPIPE just ends the feed.
        let _ = writer.write_all(&bytes);
    })?;
    Ok(Stdio::from(reader))
}

/// Reads on a helper thread: a producer stopped by Ctrl-Z would leave an inline read blocked.
pub(crate) fn read_all_watching(plan: &Plan, fd: OwnedFd, producers: Pid) -> io::Result<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().spawn_scoped(plan.scope, move || {
        let _ = tx.send(read_all(fd));
    })?;
    loop {
        match rx.recv_timeout(STOP_POLL) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {
                if group_stopped(producers) {
                    resume_group(producers);
                    plan.sink
                        .stderr_line("koa: pipelines with builtin stages cannot be suspended yet");
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("pipe reader thread failed"));
            }
        }
    }
}

fn drain<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    sink: &'env Sink,
    fd: OwnedFd,
    make: fn(Vec<u8>) -> Chunk,
) -> Result<(), LaunchError> {
    spawn_thread(scope, move || {
        let mut reader = File::from(fd);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => sink.chunk(make(buf[..n].to_vec())),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    })
}

pub(crate) fn read_all(fd: OwnedFd) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::from(fd).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn default_stdout(plan: &Plan, last: bool) -> Result<(Fd, Option<OwnedFd>), LaunchError> {
    let capture = plan.sink.is_capture();
    if last && (plan.background || !capture) {
        let fd = if capture { Fd::Null } else { Fd::Inherit(Stream::Out) };
        return Ok((fd, None));
    }
    let (reader, writer) = pipe()?;
    Ok((Fd::Owned(writer), Some(reader)))
}

fn default_stderr(plan: &Plan) -> Result<(Fd, Option<OwnedFd>), LaunchError> {
    let capture = plan.sink.is_capture();
    if plan.background || !capture {
        let fd = if capture { Fd::Null } else { Fd::Inherit(Stream::Err) };
        return Ok((fd, None));
    }
    let (reader, writer) = pipe()?;
    Ok((Fd::Owned(writer), Some(reader)))
}

pub(crate) fn launch_external(
    session: &mut Session,
    stage: &Stage,
    flow: &mut Flow,
    plan: &Plan,
    last: bool,
    launched: &mut Launched,
) -> Result<(), LaunchError> {
    let redirs = resolve_redirs(session, stage.redirs)?;

    let stdin = match redirs.stdin {
        Some(file) => {
            *flow = Flow::None;
            Stdio::from(file)
        }
        None => match std::mem::replace(flow, Flow::None) {
            Flow::Pipe(fd) => Stdio::from(fd),
            Flow::Value(value) => feed(plan.scope, value.into_raw_bytes())?,
            Flow::None if plan.background || plan.sink.is_capture() => Stdio::null(),
            Flow::None => Stdio::inherit(),
        },
    };

    let needs_default_out =
        matches!(redirs.out, OutTarget::Default) || matches!(redirs.err, ErrTarget::ToOut);
    let (default_out, out_read) = if needs_default_out {
        default_stdout(plan, last)?
    } else {
        (Fd::Null, None)
    };
    let stdout_fd = match redirs.out {
        OutTarget::File(file) => Fd::Owned(file.into()),
        OutTarget::Default => default_out
            .try_clone()
            .map_err(|e| io_error("E_PIPE", "dup stdout", &e))?,
    };
    let (stderr_fd, err_read) = match redirs.err {
        ErrTarget::File(file) => (Fd::Owned(file.into()), None),
        ErrTarget::ToOut => (
            default_out
                .try_clone()
                .map_err(|e| io_error("E_PIPE", "dup stderr", &e))?,
            None,
        ),
        ErrTarget::Default => default_stderr(plan)?,
    };

    // Readers start before the child so a failed spawn still ends them: dropping the write ends is EOF.
    let mut next_pipe = None;
    if let Some(reader) = out_read {
        if last {
            drain(plan.scope, plan.sink, reader, Chunk::Stdout)?;
        } else {
            next_pipe = Some(reader);
        }
    }
    if let Some(reader) = err_read {
        drain(plan.scope, plan.sink, reader, Chunk::Stderr)?;
    }

    // Only the terminal and the kill switch need groups; `koa -c` stays in the shell's group like `sh -c`.
    let grouped = session.interactive || plan.sink.is_capture();
    let group = match (grouped, launched.pgid) {
        (false, _) => ChildGroup::Shared,
        (true, None) => ChildGroup::Leader,
        (true, Some(pgid)) => ChildGroup::Join(pgid),
    };

    let child = spawn_external(
        session,
        ExternalSpec {
            argv: &stage.argv,
            env: &stage.env,
            stdin,
            stdout: stdout_fd.into_stdio(),
            stderr: stderr_fd.into_stdio(),
            group,
            claim_terminal: !plan.background && session.interactive && launched.pgid.is_none(),
        },
    )?;
    session.mark_opaque(stage.label.clone());

    let pid = Pid::from_raw(child.id() as i32);
    let pgid = *launched
        .pgid
        .get_or_insert(if grouped { pid } else { session.shell_pgid });
    launched.pids.push(pid);
    launched.last_pid = Some(pid);
    if grouped && !plan.background {
        session.kill.track(pgid);
        launched.tracked = Some(pgid);
    }

    *flow = match next_pipe {
        Some(reader) => Flow::Pipe(reader),
        None if last => Flow::None,
        None => Flow::Value(Value::Empty),
    };
    Ok(())
}
