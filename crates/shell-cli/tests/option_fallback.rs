#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A builtin owns the options in its signature. An option outside it runs the real
//! program of the same name when the session's PATH has one. Every case runs `koa`
//! in its own scratch directory and names every path absolutely.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const KOA: &str = env!("CARGO_BIN_EXE_koa");

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("koa-fallback-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir.canonicalize().expect("canonical scratch dir"))
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn q(&self, rel: &str) -> String {
        format!("'{}'", self.path(rel).display())
    }

    fn write(&self, rel: &str, content: &[u8]) {
        fs::write(self.path(rel), content).expect("write fixture");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_with_path(cwd: &Path, script: &str, path: Option<&Path>) -> Run {
    let mut cmd = Command::new(KOA);
    cmd.arg("-c")
        .arg(script)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let out = cmd.output().expect("run koa");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn run(cwd: &Path, script: &str) -> Run {
    run_with_path(cwd, script, None)
}

fn notices(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains("is not built in, running"))
        .collect()
}

#[test]
fn an_option_ls_lacks_runs_the_real_ls() {
    let s = Scratch::new("ls-1");
    s.write("a", b"");
    s.write("b", b"");
    let r = run(&s.0, &format!("ls -1 {}", s.q("")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "a\nb\n");
}

#[test]
fn a_fallback_stage_is_opaque_to_undo() {
    let s = Scratch::new("cp-a");
    s.write("src", b"payload");
    let r = run(
        &s.0,
        &format!(
            "echo k > {}; cp -a {} {}; undo",
            s.q("k"),
            s.q("src"),
            s.q("dst")
        ),
    );
    assert_eq!(fs::read(s.path("dst")).expect("dst copied"), b"payload");
    assert!(r.stderr.contains("E_TX_OPAQUE"), "{}", r.stderr);
    assert!(s.path("dst").exists(), "refused undo must leave the copy");
    assert!(s.path("k").exists(), "refused undo must leave earlier work");
}

#[test]
fn a_bad_value_for_a_known_option_stays_an_args_error() {
    let s = Scratch::new("head-bad-value");
    s.write("f", b"one\ntwo\n");
    let r = run(&s.0, &format!("head -n abc {}", s.q("f")));
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("E_ARGS"), "{}", r.stderr);
    assert!(notices(&r.stderr).is_empty(), "{}", r.stderr);
}

#[test]
fn without_an_executable_an_unknown_option_stays_an_args_error() {
    let s = Scratch::new("no-exec");
    let empty = Scratch::new("empty-path");
    let r = run_with_path(&s.0, "ls -1", Some(&empty.0));
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("E_ARGS"), "{}", r.stderr);
    assert!(r.stderr.contains("'-1'"), "{}", r.stderr);
}

#[test]
fn a_builtin_that_reads_dashes_as_operands_never_falls_back() {
    let s = Scratch::new("patch-dash");
    s.write("f", b"old\n");
    let table = run(&s.0, &format!("cat {}", s.q("f"))).stdout;
    let row = table.lines().last().expect("cat row");
    let hash = row.split_whitespace().nth(2).expect("line hash");
    let anchor = format!("1:{hash}");
    let r = run(&s.0, &format!("patch {} {anchor} -x", s.q("f")));
    assert!(notices(&r.stderr).is_empty(), "{}", r.stderr);
    assert_eq!(fs::read_to_string(s.path("f")).unwrap(), "-x\n");
}

#[test]
fn the_notice_is_printed_once_per_command_per_session() {
    let s = Scratch::new("notice-once");
    s.write("a", b"");
    let r = run(&s.0, &format!("ls -1 {0}; ls -1 {0}; ls -h {0}", s.q("")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "a\na\na\n");
    let lines = notices(&r.stderr);
    assert_eq!(lines.len(), 1, "{}", r.stderr);
    assert!(
        lines[0].starts_with("koa: ls: '-1' is not built in, running /")
            && lines[0].ends_with("/ls"),
        "{}",
        lines[0]
    );
}

struct RpcClient {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl RpcClient {
    fn start(cwd: &Path) -> Self {
        let mut child = Command::new(KOA)
            .arg("--rpc")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn koa --rpc");
        let stdout = child.stdout.take().expect("rpc stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines: rx }
    }

    fn send(&mut self, request: serde_json::Value) {
        let stdin = self.child.stdin.as_mut().expect("rpc stdin");
        writeln!(stdin, "{request}").expect("write rpc request");
        stdin.flush().expect("flush rpc request");
    }

    fn until_completed(&self, id: u64) -> Vec<serde_json::Value> {
        let start = Instant::now();
        let deadline = Duration::from_secs(10);
        let mut out = Vec::new();
        while let Some(left) = deadline.checked_sub(start.elapsed()) {
            let Ok(line) = self.lines.recv_timeout(left) else {
                break;
            };
            let v: serde_json::Value = serde_json::from_str(&line).expect("rpc line is json");
            let done = v["id"] == id && (v["event"] == "completed" || v.get("error").is_some());
            out.push(v);
            if done {
                break;
            }
        }
        out
    }

    fn create_session(&mut self) -> String {
        self.send(serde_json::json!({"id": 1, "method": "session.create"}));
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(5))
            .expect("session.create response");
        let v: serde_json::Value = serde_json::from_str(&line).expect("json");
        v["result"]["session_id"].as_str().expect("id").to_string()
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn over_rpc_the_notice_is_a_stderr_chunk_and_the_output_is_text() {
    let s = Scratch::new("rpc");
    s.write("a", b"");
    let mut rpc = RpcClient::start(&s.0);
    let sid = rpc.create_session();
    rpc.send(serde_json::json!({
        "id": 2,
        "method": "exec",
        "params": {"session_id": sid, "command": format!("ls -1 {}", s.q(""))}
    }));
    let events = rpc.until_completed(2);
    let stderr: String = events.iter().filter_map(|e| e["stderr"].as_str()).collect();
    let stdout: String = events.iter().filter_map(|e| e["stdout"].as_str()).collect();
    assert!(
        stderr.contains("koa: ls: '-1' is not built in, running /"),
        "{events:?}"
    );
    assert_eq!(stdout, "a\n", "{events:?}");
    assert!(
        events
            .iter()
            .all(|e| e["records"].is_null() || e.get("records").is_none()),
        "fallback output is text, not a table: {events:?}"
    );
    assert_eq!(events.last().unwrap()["exit_code"], 0, "{events:?}");
}
