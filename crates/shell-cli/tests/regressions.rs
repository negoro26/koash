#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
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
            let Ok(line) = self.lines.recv_timeout(left) else {
                break;
            };
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
    assert_eq!(
        completed["exit_code"], 124,
        "timed-out exec reports exit 124: {completed}"
    );
}

#[test]
fn builtin_to_external_to_builtin_pipeline_streams_large_input() {
    let dir = scratch_dir("deadlock");
    let body: String = (1..=50_000).map(|n| format!("{n}\n")).collect();
    std::fs::write(dir.join("big.txt"), body).expect("write big.txt");
    let out = koa_c(
        &dir,
        "cat big.txt | /bin/cat | first 1",
        Duration::from_secs(10),
    )
    .expect("pipeline larger than the pipe buffer must not hang");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("big.txt"),
        "first record expected, got: {stdout}"
    );
}

#[test]
fn builtin_failure_sets_status_and_or_list_recovers() {
    let dir = scratch_dir("or-list");
    let out = koa_c(
        &dir,
        "cat nope.txt || echo recovered",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("recovered"),
        "`||` must run after a failed builtin: {stdout}"
    );
    assert!(out.status.success(), "status of the last command is 0");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nope.txt")
            || String::from_utf8_lossy(&out.stderr).contains("E_CAT"),
        "the builtin failure is still reported on stderr"
    );

    let out = koa_c(
        &dir,
        "cat nope.txt; echo status=$?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("status=1"),
        "`$?` after a failed builtin is 1: {stdout}"
    );
}

#[test]
fn undo_after_builtin_rm_restores_the_file_and_keeps_earlier_work() {
    let dir = scratch_dir("undo-rm");
    std::fs::write(dir.join("pre.txt"), "original\n").expect("write pre.txt");
    let out = koa_c(
        &dir,
        "echo keep > keep.txt; rm pre.txt; undo",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("pre.txt")).expect("pre.txt must be restored"),
        "original\n"
    );
    assert!(
        dir.join("keep.txt").exists(),
        "undo must only revert the rm"
    );
}

#[test]
fn undo_after_builtin_rm_r_restores_the_tree() {
    let dir = scratch_dir("undo-rm-r");
    std::fs::create_dir_all(dir.join("d/sub")).expect("create tree");
    std::fs::write(dir.join("d/sub/nested.txt"), "deep\n").expect("write nested");
    let out = koa_c(&dir, "rm -r d; undo", Duration::from_secs(10)).expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert!(
        stderr.contains("E_TX_OPAQUE"),
        "undo must refuse, stderr: {stderr}"
    );
    assert!(
        dir.join("keep.txt").exists(),
        "refused undo must not touch earlier work"
    );
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
    assert!(
        dir.join("victim.txt").exists(),
        "victim.txt must survive a dry_run exec"
    );
}

#[test]
fn rpc_exec_without_policy_runs_posix_fallback() {
    let dir = scratch_dir("rpc-default-policy");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    rpc.send(serde_json::json!({
        "id": 2,
        "method": "exec",
        "params": {"session_id": sid, "command": "echo got-$(echo sub)"}
    }));
    let lines = rpc.lines_until_completed(2, Duration::from_secs(10));
    assert!(
        !lines.iter().any(|l| l.contains("policy_blocked")),
        "an exec with no policy must not be restricted: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("got-sub")),
        "command substitution output expected: {lines:?}"
    );
}

#[test]
fn undo_skips_the_empty_record_left_by_a_failed_builtin() {
    let dir = scratch_dir("undo-failed-rm");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    let mut exec = |id: u64, command: &str| {
        rpc.send(serde_json::json!({
            "id": id,
            "method": "exec",
            "params": {"session_id": sid, "command": command}
        }));
        rpc.lines_until_completed(id, Duration::from_secs(10))
    };
    exec(2, "echo keep > k.txt");
    exec(3, "rm nonexistent.txt");
    let lines = exec(4, "undo");
    assert!(
        lines.iter().any(|l| l.contains("tx_001")),
        "undo must revert the echo, not the failed rm: {lines:?}"
    );
    assert!(
        !dir.join("k.txt").exists(),
        "k.txt created by tx_001 must be gone"
    );
}

#[test]
fn rpc_child_cannot_read_the_protocol_stdin() {
    let dir = scratch_dir("rpc-stdin");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    let mut exec = |id: u64, command: &str| {
        rpc.send(serde_json::json!({
            "id": id,
            "method": "exec",
            "params": {"session_id": sid, "command": command}
        }));
        rpc.lines_until_completed(id, Duration::from_secs(10))
    };
    let first = exec(2, "/usr/bin/head -n 1");
    assert!(
        first.iter().any(|l| l.contains("\"completed\"")),
        "head must see EOF on stdin and finish: {first:?}"
    );
    let second = exec(3, "echo still-alive");
    assert!(
        second.iter().any(|l| l.contains("still-alive")),
        "the protocol stream must survive a child that reads stdin: {second:?}"
    );
}

#[test]
fn sigpipe_terminates_upstream_quietly() {
    let dir = scratch_dir("sigpipe");
    let start = Instant::now();
    let out = koa_c(
        &dir,
        "/usr/bin/yes | /usr/bin/head -n 1",
        Duration::from_secs(10),
    )
    .expect("yes | head must terminate");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
    assert!(
        out.status.success(),
        "status of head is 0: {:?}",
        out.status
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "y\n");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("Broken pipe"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn unknown_command_is_127() {
    let dir = scratch_dir("unknown-cmd");
    let out =
        koa_c(&dir, "definitely-not-a-cmd-xyz", Duration::from_secs(10)).expect("koa -c finishes");
    assert_eq!(out.status.code(), Some(127));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("definitely-not-a-cmd-xyz"),
        "stderr must name the command: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn structured_pipeline_still_works() {
    let dir = scratch_dir("structured");
    std::fs::write(dir.join("only.txt"), "x\n").expect("write only.txt");
    let out = koa_c(
        &dir,
        "ls | where kind == file | first 1",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("only.txt"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn rpc_busy_session_after_unfinished_timeout() {
    let dir = scratch_dir("rpc-busy");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    let mut exec = |id: u64, command: &str, timeout_ms: Option<u64>| {
        rpc.send(serde_json::json!({
            "id": id,
            "method": "exec",
            "params": {"session_id": sid, "command": command, "timeout_ms": timeout_ms}
        }));
        rpc.lines_until_completed(id, Duration::from_secs(10))
    };
    // The detached sleeper holds the capture pipe open past the kill, so the exec outlasts the grace period.
    // The trailing sleep keeps the group alive for a full second so setsid has left it before the kill.
    let timed_out = exec(
        2,
        "/bin/sh -c '/usr/bin/setsid -f /bin/sleep 3; exec /bin/sleep 10'",
        Some(1000),
    );
    assert!(
        timed_out.iter().any(|l| l.contains("\"exit_code\":124")),
        "timeout reports 124: {timed_out:?}"
    );
    let busy = exec(3, "echo too-soon", None);
    assert!(
        busy.iter().any(|l| l.contains("E_SESSION_BUSY")),
        "the next exec must be refused while the task is alive: {busy:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let lines = exec(4, "echo back", None);
        if lines.iter().any(|l| l.contains("back")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "session never became ready: {lines:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn exec_lines(
    rpc: &mut RpcClient,
    sid: &str,
    id: u64,
    command: &str,
    timeout_ms: Option<u64>,
) -> Vec<String> {
    rpc.send(serde_json::json!({
        "id": id,
        "method": "exec",
        "params": {"session_id": sid, "command": command, "timeout_ms": timeout_ms}
    }));
    rpc.lines_until_completed(id, Duration::from_secs(10))
}

fn stdout_of(lines: &[String]) -> String {
    lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["stdout"].as_str().map(str::to_string))
        .collect()
}

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn connect(sock: &Path) -> UnixStream {
    let start = Instant::now();
    loop {
        match UnixStream::connect(sock) {
            Ok(stream) => return stream,
            Err(e) => assert!(
                start.elapsed() < Duration::from_secs(5),
                "socket never came up: {e}"
            ),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn socket_lines_until_final(reader: &mut BufReader<UnixStream>, id: u64) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let mut line = String::new();
        if !matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
            return out;
        }
        let line = line.trim_end().to_string();
        let done = serde_json::from_str::<serde_json::Value>(&line).is_ok_and(|v| {
            v["id"] == id
                && (v["event"] == "completed"
                    || v.get("error").is_some()
                    || v.get("result").is_some())
        });
        out.push(line);
        if done {
            return out;
        }
    }
}

#[test]
fn rpc_client_hangup_after_exec_does_not_leave_the_session_busy() {
    let dir = scratch_dir("rpc-hangup");
    let sock = dir.join("koa.sock");
    let _server = KillOnDrop(
        Command::new(KOA)
            .arg("--socket")
            .arg(&sock)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn koa --socket"),
    );

    let first = connect(&sock);
    let mut reader = BufReader::new(first.try_clone().expect("clone socket"));
    let mut writer = first;
    let timeout = Some(Duration::from_secs(10));
    writer.set_read_timeout(timeout).expect("read timeout");
    let send = |writer: &mut UnixStream, request: serde_json::Value| {
        writeln!(writer, "{request}").expect("send request");
    };
    send(
        &mut writer,
        serde_json::json!({"id": 1, "method": "session.create"}),
    );
    let created = socket_lines_until_final(&mut reader, 1);
    let created: serde_json::Value =
        serde_json::from_str(created.last().expect("session.create reply")).expect("json");
    let sid = created["result"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    send(
        &mut writer,
        serde_json::json!({"id": 2, "method": "exec", "params": {"session_id": sid, "command": "echo warmup"}}),
    );
    socket_lines_until_final(&mut reader, 2);

    send(
        &mut writer,
        serde_json::json!({"id": 3, "method": "exec", "params": {"session_id": sid, "command": "echo bye"}}),
    );
    drop(reader);
    drop(writer);

    let second = connect(&sock);
    second.set_read_timeout(timeout).expect("read timeout");
    let mut reader = BufReader::new(second.try_clone().expect("clone socket"));
    let mut writer = second;
    let deadline = Instant::now() + Duration::from_secs(5);
    for id in 10.. {
        send(
            &mut writer,
            serde_json::json!({"id": id, "method": "exec", "params": {"session_id": sid, "command": "echo from-b"}}),
        );
        let lines = socket_lines_until_final(&mut reader, id);
        if lines.iter().any(|l| l.contains("from-b")) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the abandoned exec left the session busy: {lines:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn rpc_exit_ends_only_its_own_exec() {
    let dir = scratch_dir("rpc-exit");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    exec_lines(&mut rpc, &sid, 2, "exit 0", None);
    let lines = exec_lines(&mut rpc, &sid, 3, "echo a; echo b; echo c", None);
    assert_eq!(stdout_of(&lines), "a\nb\nc\n", "{lines:?}");
    let lines = exec_lines(&mut rpc, &sid, 4, "/bin/false; echo st=$?", None);
    assert_eq!(stdout_of(&lines), "st=1\n", "{lines:?}");
}

fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o7777
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn undo_restores_permission_bits() {
    let dir = scratch_dir("undo-modes");
    std::fs::write(dir.join("script.sh"), "#!/bin/sh\n").expect("write script");
    std::fs::write(dir.join("private"), "secret").expect("write private");
    std::fs::create_dir_all(dir.join("tree")).expect("create tree");
    std::fs::write(dir.join("tree/inner"), "x").expect("write inner");
    set_mode(&dir.join("script.sh"), 0o755);
    set_mode(&dir.join("private"), 0o600);
    set_mode(&dir.join("tree/inner"), 0o640);
    set_mode(&dir.join("tree"), 0o750);

    let out = koa_c(
        &dir,
        "rm script.sh; rm private; rm -r tree; undo; undo; undo",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(mode_of(&dir.join("script.sh")), 0o755);
    assert_eq!(mode_of(&dir.join("private")), 0o600);
    assert_eq!(mode_of(&dir.join("tree")), 0o750);
    assert_eq!(mode_of(&dir.join("tree/inner")), 0o640);
}

#[test]
fn writing_to_dev_null_does_not_block_undo() {
    let dir = scratch_dir("undo-dev-null");
    std::fs::write(dir.join("f1"), "orig\n").expect("write f1");
    let out = koa_c(
        &dir,
        "echo a > f1; ls > /dev/null; undo; echo st=$?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("st=0"),
        "undo must succeed: {stdout} / {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("f1")).expect("read f1"),
        "orig\n"
    );
}

#[test]
fn rollback_keeps_guarding_against_an_earlier_external_mutation() {
    let dir = scratch_dir("rollback-opaque");
    std::fs::write(dir.join("f1"), "orig\n").expect("write f1");
    let script = "echo a > f1; echo b > f2; /bin/sh -c 'echo EXTERNAL > f1'; \
                  undo; rollback tx_002; undo; echo st=$?";
    let out = koa_c(&dir, script, Duration::from_secs(10)).expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("st=1"),
        "the second undo must refuse: {stdout}"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("E_TX_OPAQUE"));
    assert_eq!(
        std::fs::read_to_string(dir.join("f1")).expect("read f1"),
        "EXTERNAL\n",
        "the external's work must survive"
    );
}

#[test]
fn failed_spawn_does_not_block_undo() {
    let dir = scratch_dir("undo-failed-spawn");
    let out = koa_c(
        &dir,
        "echo a > f1; definitely-not-a-cmd; undo; echo st=$?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("st=0"),
        "{stdout} / {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.join("f1").exists(), "undo must revert the echo");
}

fn processes_with_cmdline(needle: &str) -> Vec<u32> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let cmdline: Vec<u8> = raw.iter().map(|&b| if b == 0 { b' ' } else { b }).collect();
        if String::from_utf8_lossy(&cmdline).contains(needle) {
            found.push(pid);
        }
    }
    found
}

#[test]
fn rpc_timeout_kills_group_members_that_outlive_the_direct_child() {
    let dir = scratch_dir("rpc-leftover");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    let start = Instant::now();
    // The pid in the marker keeps concurrent test runs from seeing each other's sleepers.
    let marker = format!("30.{}", std::process::id());
    let lines = exec_lines(
        &mut rpc,
        &sid,
        2,
        &format!("/bin/sh -c '/bin/sleep {marker} & echo started'"),
        Some(500),
    );
    assert!(
        lines.iter().any(|l| l.contains("\"exit_code\":124")),
        "timeout reports 124: {lines:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
    let next = exec_lines(&mut rpc, &sid, 3, "echo next", None);
    assert_eq!(
        stdout_of(&next),
        "next\n",
        "the session must be free again: {next:?}"
    );
    assert!(
        processes_with_cmdline(&format!("sleep {marker}")).is_empty(),
        "the leftover group member must be killed"
    );
}

#[test]
fn dash_c_children_stay_in_the_shells_process_group() {
    use std::os::unix::process::CommandExt;
    let dir = scratch_dir("dash-c-group");
    let script = dir.join("pgrp.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nread -r _pid _comm _state _ppid pgrp _rest < /proc/self/stat\necho $pgrp\n",
    )
    .expect("write script");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    for command in ["./pgrp.sh", "echo hi | ./pgrp.sh"] {
        let child = Command::new(KOA)
            .arg("-c")
            .arg(command)
            .current_dir(&dir)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn koa");
        let koa_pid = child.id();
        let out = wait_with_deadline(child, Duration::from_secs(10)).expect("koa -c finishes");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            koa_pid.to_string(),
            "`{command}` must not start a new process group; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn external_in_a_deleted_directory_reports_the_directory() {
    let dir = scratch_dir("deleted-cwd");
    let out = koa_c(
        &dir,
        "mkdir d; cd d; /bin/rm -r ../d; /bin/echo still-works; echo $?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E_CWD") && stderr.contains("no longer exists"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("E_NOT_FOUND"), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1");
}

fn child_pids(pid: u32) -> Vec<u32> {
    let mut children = Vec::new();
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return children;
    };
    for task in tasks.flatten() {
        if let Ok(list) = std::fs::read_to_string(task.path().join("children")) {
            children.extend(
                list.split_whitespace()
                    .filter_map(|s| s.parse::<u32>().ok()),
            );
        }
    }
    children
}

fn is_gone(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

#[test]
fn python_worker_dies_with_koa() {
    if !have("python3") {
        return;
    }
    let dir = scratch_dir("worker-pdeathsig");
    let mut koa = Command::new(KOA)
        .arg("-c")
        .arg("@py 'import time; time.sleep(60)'")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn koa");
    let start = Instant::now();
    let worker = loop {
        if let Some(&pid) = child_pids(koa.id()).first() {
            break pid;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "worker never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    koa.kill().expect("kill koa");
    let _ = koa.wait();
    let start = Instant::now();
    while !is_gone(worker) {
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "worker {worker} outlived koa"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn workers_follow_the_session_cwd_and_see_eof_on_stdin() {
    if !have("python3") {
        return;
    }
    let dir = scratch_dir("worker-cwd");
    std::fs::create_dir_all(dir.join("a")).expect("mkdir a");
    std::fs::create_dir_all(dir.join("b")).expect("mkdir b");
    let out = koa_c(
        &dir,
        "cd a; @py 'import os; print(os.getcwd())'; cd ../b; @py 'import os; print(os.getcwd())'",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines.len() == 2 && lines[0].ends_with("/a") && lines[1].ends_with("/b"),
        "stdout: {stdout} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = koa_c(
        &dir,
        "@py 'x = input()'; echo st=$?",
        Duration::from_secs(5),
    )
    .expect("input() must see EOF instead of hanging");
    assert!(String::from_utf8_lossy(&out.stdout).contains("st=1"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("EOFError"));
}

fn koa_c_with_stdin(cwd: &Path, script: &str, stdin: &[u8], hold_open: bool) -> Option<Output> {
    let mut child = Command::new(KOA)
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn koa");
    let mut pipe = child.stdin.take().expect("koa stdin");
    let bytes = stdin.to_vec();
    std::thread::spawn(move || {
        let _ = pipe.write_all(&bytes);
        if hold_open {
            std::thread::sleep(Duration::from_secs(20));
        }
    });
    wait_with_deadline(child, Duration::from_secs(10))
}

#[test]
fn first_stage_builtin_reads_the_shells_stdin() {
    let dir = scratch_dir("builtin-stdin");
    let out = koa_c_with_stdin(&dir, "cat | /usr/bin/tr a-z A-Z", b"data\n", false)
        .expect("koa -c finishes");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "DATA\n");

    let out = koa_c_with_stdin(
        &dir,
        "from json | where a > 1",
        br#"[{"a":1},{"a":2}]"#,
        false,
    )
    .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains('2') && !stdout.contains("\n1"),
        "stdout: {stdout}"
    );
}

#[test]
fn file_operands_never_touch_the_shells_stdin() {
    let dir = scratch_dir("builtin-file-operand");
    std::fs::write(dir.join("a.txt"), "alpha\n").expect("write a.txt");
    let start = Instant::now();
    let out = koa_c_with_stdin(&dir, "cat a.txt | first 1", b"", true)
        .expect("an open, silent stdin must not block `cat a.txt`");
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(String::from_utf8_lossy(&out.stdout).contains("alpha"));
}

#[test]
fn removing_a_fifo_still_makes_undo_refuse() {
    let dir = scratch_dir("undo-fifo");
    let made = Command::new("mkfifo").arg(dir.join("p")).status();
    if !made.is_ok_and(|s| s.success()) {
        return;
    }
    let out = koa_c(
        &dir,
        "echo a > f; rm p; undo; echo st=$?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(String::from_utf8_lossy(&out.stdout).contains("st=1"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("E_TX_OPAQUE"));
}
