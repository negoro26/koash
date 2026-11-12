use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex as StdMutex, OnceLock};

use memchr::memmem;
use rusqlite::Connection;
use rustc_hash::FxHashMap;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KernelKind {
    Python,
    JavaScript,
}

struct PersistentWorker {
    _child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: ChildStdout,
    buf: Vec<u8>,
}

static WORKER_POOL: OnceLock<Mutex<FxHashMap<(usize, KernelKind), PersistentWorker>>> = OnceLock::new();
static SQLITE_POOL: OnceLock<StdMutex<FxHashMap<usize, Connection>>> = OnceLock::new();

fn session_key(session: &Session) -> usize {
    session as *const Session as usize
}

pub fn run_sql_inprocess(session: &Session, query: &str) -> Result<Value, StructuredDiagnostic> {
    let pool = SQLITE_POOL.get_or_init(|| StdMutex::new(FxHashMap::default()));
    let mut guard = pool.lock().map_err(|_| {
        StructuredDiagnostic::new("E_SQL_LOCK", "sqlite pool poisoned", None, None)
    })?;
    let conn = match guard.entry(session_key(session)) {
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

pub async fn run_persistent_worker(
    session: &Session,
    kind: KernelKind,
    code: &str,
) -> Result<Value, StructuredDiagnostic> {
    let pool = WORKER_POOL.get_or_init(|| Mutex::new(FxHashMap::default()));
    let mut guard = pool.lock().await;
    let key = (session_key(session), kind);

    if !guard.contains_key(&key) {
        let worker = spawn_worker(kind, &session.cwd)?;
        guard.insert(key, worker);
    }

    let worker = guard.get_mut(&key).ok_or_else(|| {
        StructuredDiagnostic::new("E_SUBKERNEL", "worker missing", None, None)
    })?;

    let sentinel = format!("__KOA_EOF_{}__", std::process::id());
    let code_json = serde_json::to_string(code).unwrap_or_else(|_| "\"\"".to_string());
    let sent_json = serde_json::to_string(&sentinel).unwrap_or_else(|_| "\"\"".to_string());
    let payload = match kind {
        KernelKind::Python => format!(
            "exec(\"try:\\n exec(compile(__import__('json').loads({}),'<koa>','exec'))\\nexcept Exception as _e:\\n print(_e,file=__import__('sys').stderr)\\nprint({},flush=True)\")\n",
            serde_json::to_string(&code_json).unwrap_or_default(),
            serde_json::to_string(&sent_json).unwrap_or_default()
        ),
        KernelKind::JavaScript => format!(
            "try {{ eval(JSON.parse({code_json})); }} catch (e) {{ console.error(e); }} console.log({sent_json});\n"
        ),
    };

    if worker.stdin.write_all(payload.as_bytes()).await.is_err()
        || worker.stdin.flush().await.is_err()
    {
        guard.remove(&key);
        return Err(StructuredDiagnostic::new(
            "E_SUBKERNEL",
            "worker pipe broken",
            None,
            None,
        ));
    }

    worker.buf.clear();
    let finder = memmem::Finder::new(sentinel.as_bytes());
    let mut tmp = [0u8; 4096];

    loop {
        if let Some(pos) = finder.find(&worker.buf) {
            let output = worker.buf[..pos].to_vec();
            return Ok(Value::from_bytes(output));
        }
        let n = worker
            .stdout
            .read(&mut tmp)
            .await
            .map_err(|e| StructuredDiagnostic::from_io("E_SUBKERNEL_READ", &e, None))?;
        if n == 0 {
            let out = std::mem::take(&mut worker.buf);
            guard.remove(&key);
            return Ok(Value::from_bytes(out));
        }
        worker.buf.extend_from_slice(&tmp[..n]);
    }
}

fn spawn_worker(kind: KernelKind, cwd: &Path) -> Result<PersistentWorker, StructuredDiagnostic> {
    let (bin, args): (&str, &[&str]) = match kind {
        KernelKind::Python => (
            "python3",
            &[
                "-u",
                "-c",
                "import sys\nwhile True:\n line = sys.stdin.readline()\n if not line: break\n exec(compile(line, '<koa>', 'exec'))",
            ],
        ),
        KernelKind::JavaScript => (
            "node",
            &[
                "-e",
                "const rl=require('readline').createInterface({input:process.stdin});rl.on('line',l=>{eval(l);});",
            ],
        ),
    };

    let mut child = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| StructuredDiagnostic::new("E_SUBKERNEL", format!("{bin}: {e}"), None, None))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| StructuredDiagnostic::new("E_SUBKERNEL", "missing stdin", None, None))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| StructuredDiagnostic::new("E_SUBKERNEL", "missing stdout", None, None))?;

    Ok(PersistentWorker {
        _child: child,
        stdin,
        stdout,
        buf: Vec::with_capacity(4096),
    })
}
