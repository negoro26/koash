use std::fs::File;
use std::io::{PipeWriter, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, mpsc};

use memchr::{memchr, memmem};
use nix::libc;
use nix::unistd::Pid;
use rusqlite::Connection;
use rustc_hash::FxHashMap;
use serde_json::{Value as JsonVal, json};
use shell_kernel::{ChildGroup, ChildSetup, Session, prepare_child};
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KernelKind {
    Python,
    JavaScript,
}

impl KernelKind {
    fn code(self) -> &'static str {
        match self {
            KernelKind::Python => "E_PY",
            KernelKind::JavaScript => "E_JS",
        }
    }
}

struct Worker {
    child: Child,
    requests: PipeWriter,
    replies: File,
    buf: Vec<u8>,
}

static WORKERS: LazyLock<Mutex<FxHashMap<(u64, KernelKind), Worker>>> =
    LazyLock::new(|| Mutex::new(FxHashMap::default()));
static SQLITE_POOL: LazyLock<Mutex<FxHashMap<u64, Connection>>> =
    LazyLock::new(|| Mutex::new(FxHashMap::default()));
static REQUESTS: AtomicU64 = AtomicU64::new(0);

const PYTHON_WORKER: &str = "\
import sys, os, json, traceback
g = {'__name__': '__main__'}
for line in os.fdopen(3):
    r = json.loads(line)
    try:
        os.chdir(r['cwd'])
        exec(compile(r['code'], '<koa>', 'exec'), g)
        s = 'OK'
    except BaseException as e:
        s = 'ERR ' + json.dumps(''.join(traceback.format_exception(type(e), e, e.__traceback__.tb_next)))
    sys.stdout.write(r['end'] + s + '\\n')
    sys.stdout.flush()
";

const JAVASCRIPT_WORKER: &str = "\
const rl = require('readline').createInterface({ input: require('fs').createReadStream(null, { fd: 3 }) });
rl.on('line', l => {
  const r = JSON.parse(l);
  let s = 'OK';
  try { process.chdir(r.cwd); (0, eval)(r.code); } catch (e) { s = 'ERR ' + JSON.stringify(String(e)); }
  process.stdout.write(r.end + s + '\\n');
});
";

fn pool_error(what: &str) -> StructuredDiagnostic {
    StructuredDiagnostic::new("E_SUBKERNEL", format!("{what} pool poisoned"), None, None)
}

pub fn run_sql_inprocess(session: &Session, query: &str) -> Result<Value, StructuredDiagnostic> {
    let mut guard = SQLITE_POOL.lock().map_err(|_| pool_error("sqlite"))?;
    let conn = match guard.entry(session.uid) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => {
            let c = Connection::open_in_memory()
                .map_err(|err| StructuredDiagnostic::new("E_SQL", err.to_string(), None, None))?;
            e.insert(c)
        }
    };

    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Value::Empty);
    }

    let upper = trimmed.get(..6).unwrap_or(trimmed).to_ascii_uppercase();
    if upper.starts_with("SELECT") || upper.starts_with("PRAGMA") || upper.starts_with("WITH") {
        let mut stmt = conn
            .prepare_cached(trimmed)
            .map_err(|e| StructuredDiagnostic::new("E_SQL", e.to_string(), None, None))?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| (*s).to_string()).collect();
        let mut rows = stmt
            .query([])
            .map_err(|e| StructuredDiagnostic::new("E_SQL", e.to_string(), None, None))?;
        let mut records = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StructuredDiagnostic::new("E_SQL", e.to_string(), None, None))?
        {
            let mut rec = StructuredRecord::new();
            for (idx, col) in col_names.iter().enumerate() {
                let val = match row.get_ref(idx) {
                    Ok(rusqlite::types::ValueRef::Null) => JsonVal::Null,
                    Ok(rusqlite::types::ValueRef::Integer(i)) => json!(i),
                    Ok(rusqlite::types::ValueRef::Real(f)) => json!(f),
                    Ok(rusqlite::types::ValueRef::Text(t)) => {
                        JsonVal::String(String::from_utf8_lossy(t).to_string())
                    }
                    Ok(rusqlite::types::ValueRef::Blob(b)) => {
                        JsonVal::String(format!("<blob:{}>", b.len()))
                    }
                    Err(_) => JsonVal::Null,
                };
                rec.insert(col.clone(), val);
            }
            records.push(rec);
        }
        Ok(Value::from_records(records))
    } else {
        conn.execute_batch(trimmed)
            .map_err(|e| StructuredDiagnostic::new("E_SQL", e.to_string(), None, None))?;
        Ok(Value::Empty)
    }
}

/// Runs `code` in the session's persistent worker for `kind`. A worker is out of the pool while it
/// runs, so slow snippets in one session never block another. An exception in user code is an
/// error diagnostic and the worker stays alive; a dead worker is dropped and respawned next time.
pub fn run_worker(
    session: &Session,
    kind: KernelKind,
    code: &str,
) -> Result<Value, StructuredDiagnostic> {
    let key = (session.uid, kind);
    let pooled = WORKERS.lock().map_err(|_| pool_error("worker"))?.remove(&key);
    let mut worker = match pooled {
        Some(w) => w,
        None => spawn_worker(kind)?,
    };

    // In an interactive shell the worker owns the terminal for the call, so Ctrl-C reaches it
    // and not the shell.
    let pgid = Pid::from_raw(worker.child.id() as i32);
    session.kill.track(pgid);
    session.give_terminal(pgid);
    let reply = exchange(&mut worker, code, &session.cwd);
    session.restore_terminal();
    session.kill.untrack(pgid);

    match reply {
        Ok(Reply::Done(output)) => {
            WORKERS.lock().map_err(|_| pool_error("worker"))?.insert(key, worker);
            Ok(if output.is_empty() {
                Value::Empty
            } else {
                Value::from_bytes(output)
            })
        }
        Ok(Reply::Raised(text)) => {
            WORKERS.lock().map_err(|_| pool_error("worker"))?.insert(key, worker);
            Err(StructuredDiagnostic::new(kind.code(), text, None, None))
        }
        Err(diag) => {
            let _ = worker.child.kill();
            let status = worker.child.wait();
            if session.kill.is_killed() {
                return Err(StructuredDiagnostic::new("E_KILLED", "command killed", None, None));
            }
            if status.is_ok_and(|s| s.signal() == Some(libc::SIGINT)) {
                return Err(StructuredDiagnostic::new(kind.code(), "interrupted", None, None));
            }
            Err(diag)
        }
    }
}

enum Reply {
    Done(Vec<u8>),
    Raised(String),
}

fn exchange(worker: &mut Worker, code: &str, cwd: &Path) -> Result<Reply, StructuredDiagnostic> {
    let end = format!(
        "__KOA_END_{}_{}__",
        std::process::id(),
        REQUESTS.fetch_add(1, Ordering::Relaxed)
    );
    let cwd = cwd.to_string_lossy();
    let mut request = json!({ "code": code, "end": end, "cwd": cwd }).to_string();
    request.push('\n');
    worker
        .requests
        .write_all(request.as_bytes())
        .and_then(|()| worker.requests.flush())
        .map_err(|e| StructuredDiagnostic::from_io("E_SUBKERNEL", &e, None))?;

    worker.buf.clear();
    let finder = memmem::Finder::new(end.as_bytes());
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(pos) = finder.find(&worker.buf) {
            let after = pos + end.len();
            if let Some(nl) = memchr(b'\n', &worker.buf[after..]) {
                let status = String::from_utf8_lossy(&worker.buf[after..after + nl]).into_owned();
                let mut output = std::mem::take(&mut worker.buf);
                output.truncate(pos);
                return Ok(match status.strip_prefix("ERR ") {
                    Some(json_text) => Reply::Raised(
                        serde_json::from_str::<String>(json_text)
                            .unwrap_or_else(|_| json_text.to_string())
                            .trim_end()
                            .to_string(),
                    ),
                    None => Reply::Done(output),
                });
            }
        }
        match worker.replies.read(&mut chunk) {
            Ok(0) => {
                return Err(StructuredDiagnostic::new(
                    "E_SUBKERNEL",
                    "worker exited before replying",
                    None,
                    None,
                ));
            }
            Ok(n) => worker.buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(StructuredDiagnostic::from_io("E_SUBKERNEL_READ", &e, None)),
        }
    }
}

type SpawnJob = (Command, mpsc::Sender<std::io::Result<Child>>);

/// Workers are spawned from one thread that lives as long as the process. PDEATHSIG fires when the
/// thread that forked the child exits, and a runtime's blocking-pool threads exit when idle.
static SPAWNER: LazyLock<Option<mpsc::Sender<SpawnJob>>> = LazyLock::new(|| {
    let (jobs, queue) = mpsc::channel::<SpawnJob>();
    std::thread::Builder::new()
        .name("koa-worker-spawner".to_string())
        .spawn(move || {
            for (mut command, reply) in queue {
                let _ = reply.send(command.spawn());
            }
        })
        .ok()?;
    Some(jobs)
});

fn spawn_on_spawner(command: Command) -> std::io::Result<Child> {
    let unavailable = || std::io::Error::other("worker spawner unavailable");
    let (reply, spawned) = mpsc::channel();
    SPAWNER
        .as_ref()
        .ok_or_else(unavailable)?
        .send((command, reply))
        .map_err(|_| unavailable())?;
    spawned.recv().map_err(|_| unavailable())?
}

/// Workers read requests from fd 3 and have /dev/null as stdin, so user code that reads stdin
/// sees EOF instead of eating the protocol.
const REQUEST_FD: i32 = 3;

fn spawn_worker(kind: KernelKind) -> Result<Worker, StructuredDiagnostic> {
    let (bin, flags, script): (&str, &[&str], &str) = match kind {
        KernelKind::Python => ("python3", &["-u", "-c"], PYTHON_WORKER),
        KernelKind::JavaScript => ("node", &["-e"], JAVASCRIPT_WORKER),
    };
    let fail = |what: String| StructuredDiagnostic::new("E_SUBKERNEL", what, None, None);

    let (requests_in, requests) = std::io::pipe().map_err(|e| fail(format!("{bin}: pipe: {e}")))?;
    let (replies, replies_out) = std::io::pipe().map_err(|e| fail(format!("{bin}: pipe: {e}")))?;
    let replies_err = replies_out
        .try_clone()
        .map_err(|e| fail(format!("{bin}: pipe: {e}")))?;

    let request_fd = requests_in.as_raw_fd();
    let mut command = Command::new(bin);
    command
        .args(flags)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::from(replies_out))
        .stderr(Stdio::from(replies_err));
    prepare_child(
        &mut command,
        ChildSetup {
            group: ChildGroup::Leader,
            tty: None,
            die_with_parent: true,
        },
    );
    // SAFETY: only dup2 and fcntl run between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if request_fd == REQUEST_FD {
                // dup2 onto itself would leave close-on-exec set.
                if libc::fcntl(REQUEST_FD, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(request_fd, REQUEST_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    // `command` owns the parent's copies of the reply write ends and is dropped by the spawner
    // thread; without that a dead worker would never produce EOF.
    let child = spawn_on_spawner(command).map_err(|e| fail(format!("{bin}: {e}")))?;
    drop(requests_in);

    Ok(Worker {
        child,
        requests,
        replies: File::from(std::os::fd::OwnedFd::from(replies)),
        buf: Vec::with_capacity(4096),
    })
}
