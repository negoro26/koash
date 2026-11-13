use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use memchr::{memchr, memmem};
use nix::unistd::Pid;
use rusqlite::Connection;
use rustc_hash::FxHashMap;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
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
    stdin: ChildStdin,
    stdout: File,
    buf: Vec<u8>,
}

static WORKERS: LazyLock<Mutex<FxHashMap<(u64, KernelKind), Worker>>> =
    LazyLock::new(|| Mutex::new(FxHashMap::default()));
static SQLITE_POOL: LazyLock<Mutex<FxHashMap<u64, Connection>>> =
    LazyLock::new(|| Mutex::new(FxHashMap::default()));
static REQUESTS: AtomicU64 = AtomicU64::new(0);

const PYTHON_WORKER: &str = "\
import sys, json, traceback
g = {'__name__': '__main__'}
for line in sys.stdin:
    r = json.loads(line)
    try:
        exec(compile(r['code'], '<koa>', 'exec'), g)
        s = 'OK'
    except BaseException as e:
        s = 'ERR ' + json.dumps(''.join(traceback.format_exception(type(e), e, e.__traceback__.tb_next)))
    sys.stdout.write(r['end'] + s + '\\n')
    sys.stdout.flush()
";

const JAVASCRIPT_WORKER: &str = "\
const rl = require('readline').createInterface({ input: process.stdin });
rl.on('line', l => {
  const r = JSON.parse(l);
  let s = 'OK';
  try { (0, eval)(r.code); } catch (e) { s = 'ERR ' + JSON.stringify(String(e)); }
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
        None => spawn_worker(kind, &session.cwd)?,
    };

    let pgid = Pid::from_raw(worker.child.id() as i32);
    session.kill.track(pgid);
    let reply = exchange(&mut worker, code);
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
            let _ = worker.child.wait();
            if session.kill.is_killed() {
                return Err(StructuredDiagnostic::new("E_KILLED", "command killed", None, None));
            }
            Err(diag)
        }
    }
}

enum Reply {
    Done(Vec<u8>),
    Raised(String),
}

fn exchange(worker: &mut Worker, code: &str) -> Result<Reply, StructuredDiagnostic> {
    let end = format!(
        "__KOA_END_{}_{}__",
        std::process::id(),
        REQUESTS.fetch_add(1, Ordering::Relaxed)
    );
    let mut request = json!({ "code": code, "end": end }).to_string();
    request.push('\n');
    worker
        .stdin
        .write_all(request.as_bytes())
        .and_then(|()| worker.stdin.flush())
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
        match worker.stdout.read(&mut chunk) {
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

fn spawn_worker(kind: KernelKind, cwd: &Path) -> Result<Worker, StructuredDiagnostic> {
    let (bin, flags, script): (&str, &[&str], &str) = match kind {
        KernelKind::Python => ("python3", &["-u", "-c"], PYTHON_WORKER),
        KernelKind::JavaScript => ("node", &["-e"], JAVASCRIPT_WORKER),
    };
    let fail = |what: String| StructuredDiagnostic::new("E_SUBKERNEL", what, None, None);

    let (reader, writer) = std::io::pipe().map_err(|e| fail(format!("{bin}: pipe: {e}")))?;
    let writer_err = writer
        .try_clone()
        .map_err(|e| fail(format!("{bin}: pipe: {e}")))?;

    // The Command owns the parent's copies of the write ends; it must drop before we read,
    // or a dead worker would never produce EOF.
    let mut child = {
        let mut cmd = Command::new(bin);
        cmd.args(flags)
            .arg(script)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(writer))
            .stderr(Stdio::from(writer_err))
            .process_group(0);
        cmd.spawn().map_err(|e| fail(format!("{bin}: {e}")))?
    };

    let stdin = child.stdin.take().ok_or_else(|| fail("missing stdin".to_string()))?;
    Ok(Worker {
        child,
        stdin,
        stdout: File::from(std::os::fd::OwnedFd::from(reader)),
        buf: Vec::with_capacity(4096),
    })
}
