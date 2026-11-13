use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const KOA: &str = env!("CARGO_BIN_EXE_koa");

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("koa-regress-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn wait_with_deadline(mut child: Child, deadline: Duration) -> Option<Output> {
    let start = Instant::now();
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            return Some(child.wait_with_output().expect("collect output"));
        }
        if start.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn koa_c(cwd: &Path, script: &str, deadline: Duration) -> Option<Output> {
    let child = Command::new(KOA)
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn koa");
    wait_with_deadline(child, deadline)
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

    fn lines_until_completed(&self, id: u64, deadline: Duration) -> Vec<String> {
        let start = Instant::now();
        let mut out = Vec::new();
        while let Some(left) = deadline.checked_sub(start.elapsed()) {
            let Ok(line) = self.lines.recv_timeout(left) else { break };
            let done = serde_json::from_str::<serde_json::Value>(&line).is_ok_and(|v| {
                v["id"] == id && (v["event"] == "completed" || v.get("error").is_some())
            });
            out.push(line);
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
        let v: serde_json::Value = serde_json::from_str(&line).expect("session.create is json");
        v["result"]["session_id"]
            .as_str()
            .expect("session_id in response")
            .to_string()
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn rpc_external_output_never_corrupts_the_protocol_stream() {
    let dir = scratch_dir("rpc-leak");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    rpc.send(serde_json::json!({
        "id": 2,
        "method": "exec",
        "params": {"session_id": sid, "command": "/bin/echo LEAKED"}
    }));
    let lines = rpc.lines_until_completed(2, Duration::from_secs(10));
    for line in &lines {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "non-JSON line on the RPC stream: {line:?}"
        );
    }
    assert!(
        lines.iter().any(|l| l.contains("LEAKED")),
        "external output must be delivered inside a protocol message: {lines:?}"
    );
}

#[test]
fn rpc_timeout_kills_a_long_external_command() {
    let dir = scratch_dir("rpc-timeout");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    let start = Instant::now();
    rpc.send(serde_json::json!({
        "id": 2,
        "method": "exec",
        "params": {"session_id": sid, "command": "sleep 5", "timeout_ms": 200}
    }));
    let lines = rpc.lines_until_completed(2, Duration::from_secs(10));
    let elapsed = start.elapsed();
    let completed: serde_json::Value = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["event"] == "completed" || v.get("error").is_some())
        .expect("exec must finish with a completed event or an error");
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout_ms=200 must stop `sleep 5`, took {elapsed:?}"
    );
    assert_eq!(completed["exit_code"], 124, "timed-out exec reports exit 124: {completed}");
}

#[test]
fn builtin_to_external_to_builtin_pipeline_streams_large_input() {
    let dir = scratch_dir("deadlock");
    let body: String = (1..=50_000).map(|n| format!("{n}\n")).collect();
    std::fs::write(dir.join("big.txt"), body).expect("write big.txt");
    let out = koa_c(&dir, "cat big.txt | /bin/cat | first 1", Duration::from_secs(10))
        .expect("pipeline larger than the pipe buffer must not hang");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("big.txt"), "first record expected, got: {stdout}");
}

#[test]
fn builtin_failure_sets_status_and_or_list_recovers() {
    let dir = scratch_dir("or-list");
    let out = koa_c(&dir, "cat nope.txt || echo recovered", Duration::from_secs(10))
        .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("recovered"), "`||` must run after a failed builtin: {stdout}");
    assert!(out.status.success(), "status of the last command is 0");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nope.txt")
            || String::from_utf8_lossy(&out.stderr).contains("E_CAT"),
        "the builtin failure is still reported on stderr"
    );

    let out = koa_c(&dir, "cat nope.txt; echo status=$?", Duration::from_secs(10))
        .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("status=1"), "`$?` after a failed builtin is 1: {stdout}");
}

#[test]
fn undo_after_builtin_rm_restores_the_file_and_keeps_earlier_work() {
    let dir = scratch_dir("undo-rm");
    std::fs::write(dir.join("pre.txt"), "original\n").expect("write pre.txt");
    let out = koa_c(&dir, "echo keep > keep.txt; rm pre.txt; undo", Duration::from_secs(10))
        .expect("koa -c finishes");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(dir.join("pre.txt")).expect("pre.txt must be restored"),
        "original\n"
    );
    assert!(dir.join("keep.txt").exists(), "undo must only revert the rm");
}

#[test]
fn undo_after_builtin_rm_r_restores_the_tree() {
    let dir = scratch_dir("undo-rm-r");
    std::fs::create_dir_all(dir.join("d/sub")).expect("create tree");
    std::fs::write(dir.join("d/sub/nested.txt"), "deep\n").expect("write nested");
    let out = koa_c(&dir, "rm -r d; undo", Duration::from_secs(10)).expect("koa -c finishes");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(dir.join("d/sub/nested.txt")).expect("nested file restored"),
        "deep\n"
    );
}

#[test]
fn undo_refuses_after_external_mutation() {
    let dir = scratch_dir("undo-opaque");
    std::fs::write(dir.join("pre.txt"), "original\n").expect("write pre.txt");
    let out = koa_c(
        &dir,
        "echo keep > keep.txt; /bin/rm pre.txt; undo",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E_TX_OPAQUE"), "undo must refuse, stderr: {stderr}");
    assert!(dir.join("keep.txt").exists(), "refused undo must not touch earlier work");
}

#[test]
fn rpc_policy_blocks_posix_fallback_under_dry_run() {
    let dir = scratch_dir("rpc-policy");
    std::fs::write(dir.join("victim.txt"), "keep me\n").expect("write victim");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    rpc.send(serde_json::json!({
        "id": 2,
        "method": "exec",
        "params": {
            "session_id": sid,
            "command": "rm victim.txt $(true)",
            "policy": {"dry_run": true}
        }
    }));
    let lines = rpc.lines_until_completed(2, Duration::from_secs(10));
    assert!(
        lines.iter().any(|l| l.contains("policy_blocked")),
        "dry_run must block an opaque construct: {lines:?}"
    );
    assert!(dir.join("victim.txt").exists(), "victim.txt must survive a dry_run exec");
}
