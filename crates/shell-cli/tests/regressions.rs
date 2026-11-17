#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const KOA: &str = env!("CARGO_BIN_EXE_koa");

struct ScratchDir(PathBuf);

impl std::ops::Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch_dir(name: &str) -> ScratchDir {
    let dir = std::env::temp_dir().join(format!("koa-regress-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    ScratchDir(dir)
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
    let lines = exec_lines(&mut rpc, &sid, 2, "/bin/echo LEAKED", None);
    for line in &lines {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "non-JSON line on the RPC stream: {line:?}"
        );
    }
    assert_eq!(
        stdout_of(&lines),
        "LEAKED\n",
        "external output must be delivered inside protocol messages: {lines:?}"
    );
}

#[test]
fn rpc_timeout_kills_a_long_external_command() {
    let dir = scratch_dir("rpc-timeout");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    let marker = format!("31.{}", std::process::id());
    let start = Instant::now();
    let lines = exec_lines(
        &mut rpc,
        &sid,
        2,
        &format!("/bin/sleep {marker}"),
        Some(200),
    );
    let elapsed = start.elapsed();
    let completed: serde_json::Value = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["event"] == "completed" || v.get("error").is_some())
        .expect("exec must finish with a completed event or an error");
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout_ms=200 must stop the sleep, took {elapsed:?}"
    );
    assert_eq!(
        completed["exit_code"], 124,
        "timed-out exec reports exit 124: {completed}"
    );
    let next = exec_lines(&mut rpc, &sid, 3, "echo next", None);
    assert_eq!(
        stdout_of(&next),
        "next\n",
        "session is free again: {next:?}"
    );
    assert!(
        processes_with_cmdline(&format!("sleep {marker}")).is_empty(),
        "the timed-out command must be dead, not left running"
    );
}

#[test]
fn builtin_to_external_to_builtin_pipeline_streams_large_input() {
    let dir = scratch_dir("deadlock");
    let body: String = (1..=50_000).map(|n| format!("{n}\n")).collect();
    std::fs::write(dir.join("big.txt"), body).expect("write big.txt");
    let out = koa_c(
        &dir,
        "cat big.txt | /bin/cat | last 1",
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
        stdout.contains("50000") && !stdout.contains("49999"),
        "last line expected, got: {stdout}"
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
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "status=1\n",
        "`$?` after a failed builtin is exactly 1"
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
    std::fs::write(dir.join("a.txt"), "x\n").expect("write a.txt");
    std::fs::write(dir.join("b.txt"), "x\n").expect("write b.txt");
    std::fs::create_dir(dir.join("d")).expect("mkdir d");
    let names = |script: &str| -> Vec<String> {
        let out = koa_c(&dir, script, Duration::from_secs(10)).expect("koa -c finishes");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let rows: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json rows");
        let mut names: Vec<String> = rows
            .as_array()
            .expect("array of rows")
            .iter()
            .map(|row| row["name"].as_str().expect("name column").to_string())
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        names("ls | where kind == file | select name | to json"),
        ["a.txt", "b.txt"]
    );
    assert_eq!(
        names("ls | where kind == file | first 1 | select name | to json").len(),
        1
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
    assert!(stderr.contains("E_CWD"), "stderr: {stderr}");
    assert!(
        stderr.contains(&*dir.join("d").to_string_lossy()),
        "the error names the deleted directory: {stderr}"
    );
    assert!(!stderr.contains("E_NOT_FOUND"), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
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

    let out = koa_c_with_stdin(&dir, "@py 'x = input()'; echo st=$?", b"", true)
        .expect("input() must see EOF even though koa's own stdin is an open pipe");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "st=1\n");
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

fn undo_store_sessions(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join(".koa/undo"))
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

fn out_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn redo_after_undo_restores_the_post_edit_content() {
    let dir = scratch_dir("redo-basic");
    let out = koa_c(
        &dir,
        "echo first > f; echo second > f; undo; cat f; redo; cat f",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = out_text(&out);
    let after_undo = stdout
        .find("first")
        .expect("undo shows the earlier content");
    let after_redo = stdout
        .rfind("second")
        .expect("redo shows the edited content");
    assert!(after_undo < after_redo, "stdout: {stdout}");
    assert_eq!(
        std::fs::read_to_string(dir.join("f")).expect("f exists"),
        "second\n"
    );
}

#[test]
fn a_new_edit_after_undo_clears_redo() {
    let dir = scratch_dir("redo-cleared");
    let out = koa_c(
        &dir,
        "echo a > f; echo b > f; undo; echo c > f; redo; echo redo-status=$?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out_text(&out).contains("redo-status=0"));
    assert_eq!(
        std::fs::read_to_string(dir.join("f")).expect("f exists"),
        "c\n",
        "redo must find nothing to re-apply after a new edit"
    );
}

#[test]
fn redo_after_an_external_write_never_overwrites_it() {
    let dir = scratch_dir("redo-external");
    let out = koa_c(
        &dir,
        "echo a > f; echo b > f; undo; /bin/sh -c 'echo ext > f'; redo",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("f")).expect("f exists"),
        "ext\n"
    );
}

#[test]
fn redo_reapplies_a_rollback_and_the_ledger_is_whole_again() {
    let dir = scratch_dir("redo-rollback");
    let out = koa_c(
        &dir,
        "echo 1 > f; echo 2 > f; echo 3 > f; rollback tx_002; redo; undo",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("f")).expect("f exists"),
        "2\n",
        "redo must bring back tx_002 and tx_003, so undo reverts only tx_003"
    );
}

#[test]
fn tx_log_reports_pending_redo() {
    let dir = scratch_dir("redo-tx-log");
    let out = koa_c(
        &dir,
        "echo a > f; echo b > f; undo; tx-log | select redo | to json",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out_text(&out).contains("\"redo\": 1"),
        "stdout: {}",
        out_text(&out)
    );
}

#[test]
fn tx_log_reports_pending_redo_when_the_ledger_is_empty() {
    let dir = scratch_dir("redo-tx-log-empty");
    let out = koa_c(
        &dir,
        "echo a > f; undo; echo tx-log-follows; tx-log | select redo | to json",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = out_text(&out);
    let (_, log) = stdout.split_once("tx-log-follows\n").expect("marker line");
    let rows: serde_json::Value = serde_json::from_str(log)
        .unwrap_or_else(|e| panic!("tx-log output is JSON ({e}): {stdout}"));
    assert!(
        rows.as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["redo"] == 1)),
        "undo left one redo pending, so tx-log must say so: {rows}"
    );
}

#[test]
fn undo_after_rm_of_a_large_file_restores_it_byte_for_byte() {
    let dir = scratch_dir("undo-large");
    let bytes: Vec<u8> = (0..50 * 1024 * 1024_u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    std::fs::write(dir.join("big"), &bytes).expect("write big");
    let out = koa_c(&dir, "rm big; undo", Duration::from_secs(60)).expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        std::fs::read(dir.join("big")).expect("big restored") == bytes,
        "restored file differs from the original"
    );
}

fn filesystem_type(dir: &Path) -> u64 {
    let out = Command::new("stat")
        .args(["-f", "-c", "%t"])
        .arg(dir)
        .output()
        .expect("run stat");
    assert!(out.status.success(), "stat -f failed on {}", dir.display());
    u64::from_str_radix(String::from_utf8_lossy(&out.stdout).trim(), 16).expect("hex f_type")
}

const BTRFS_MAGIC: u64 = 0x9123_683E;
const XFS_MAGIC: u64 = 0x5846_5342;

#[test]
fn a_file_over_the_copy_budget_is_undoable_exactly_when_the_filesystem_can_reflink() {
    let base =
        std::env::var_os("KOA_TEST_REFLINK_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let dir = base.join(format!("koa-regress-{}-reflink", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let _cleanup = ScratchDir(dir.clone());
    let can_reflink = matches!(filesystem_type(&dir), BTRFS_MAGIC | XFS_MAGIC);
    assert!(
        can_reflink || std::env::var_os("KOA_TEST_REFLINK_DIR").is_none(),
        "KOA_TEST_REFLINK_DIR must point at btrfs or XFS, found f_type {:#x}",
        filesystem_type(&dir)
    );

    let bytes: Vec<u8> = (0..80 * 1024 * 1024_u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    std::fs::write(dir.join("big"), &bytes).expect("write big");
    let out = koa_c(
        &dir,
        "echo earlier > f; rm big; undo; echo st=$?; cat f",
        Duration::from_secs(60),
    )
    .expect("koa -c finishes");
    let stdout = out_text(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if can_reflink {
        assert!(
            stdout.contains("st=0"),
            "stdout: {stdout}\nstderr: {stderr}"
        );
        assert!(
            std::fs::read(dir.join("big")).expect("big restored") == bytes,
            "the restored file differs from the original"
        );
    } else {
        assert!(
            stderr.contains("E_TX_OPAQUE"),
            "undo must refuse a 80 MiB file it can only copy, stderr: {stderr}"
        );
        assert!(stdout.contains("st=1"), "stdout: {stdout}");
        assert!(
            !dir.join("big").exists(),
            "a refused undo must not bring the file back half-way"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("f")).expect("earlier work stays"),
            "earlier\n"
        );
    }
}

#[test]
fn rm_recursive_refuses_the_workspace_root_of_an_isolated_fork() {
    let scratch = scratch_dir("rm-workspace-root");
    let dir = scratch.canonicalize().expect("canonical scratch dir");
    std::fs::write(dir.join("keep.txt"), "precious").expect("write keep.txt");
    std::fs::create_dir(dir.join("sub")).expect("create sub");
    std::fs::write(dir.join("sub/inner.txt"), "inner").expect("write inner.txt");
    let mut rpc = RpcClient::start(&dir);
    let parent = rpc.create_session();
    rpc.send(serde_json::json!({
        "id": 2,
        "method": "session.fork",
        "params": {"session_id": parent, "isolated": true}
    }));
    let fork_line = rpc
        .lines
        .recv_timeout(Duration::from_secs(5))
        .expect("session.fork response");
    let fork: serde_json::Value = serde_json::from_str(&fork_line).expect("session.fork is json");
    let sid = fork["result"]["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("session_id in {fork_line}"))
        .to_string();

    for (id, root) in [
        (3, dir.display().to_string()),
        (4, format!("{}/", dir.display())),
    ] {
        rpc.send(serde_json::json!({
            "id": id,
            "method": "exec",
            "params": {"session_id": sid, "command": format!("rm -r '{root}'")}
        }));
        let events: Vec<serde_json::Value> = rpc
            .lines_until_completed(id, Duration::from_secs(10))
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let completed = events
            .iter()
            .find(|v| v["event"] == "completed")
            .unwrap_or_else(|| panic!("no completed event for {root}: {events:?}"));
        assert_eq!(completed["exit_code"], 1, "{root}: {events:?}");
        let stderr: String = events.iter().filter_map(|v| v["stderr"].as_str()).collect();
        assert!(stderr.contains("E_RM"), "{root}: {stderr}");
        assert!(stderr.contains("workspace root"), "{root}: {stderr}");
        assert!(!stderr.contains("filesystem root"), "{root}: {stderr}");
        assert_eq!(
            std::fs::read_to_string(dir.join("keep.txt")).expect("keep.txt survives"),
            "precious",
            "{root}"
        );
        assert!(dir.join("sub/inner.txt").exists(), "{root}");
    }
}

fn koa_stdout(dir: &Path, script: &str) -> String {
    let out = koa_c(dir, script, Duration::from_secs(10)).expect("koa -c finishes");
    assert!(
        out.status.success(),
        "`{script}` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 stdout")
}

fn json_strings(json: &serde_json::Value, key: &str) -> Vec<String> {
    json.as_array()
        .expect("a json array")
        .iter()
        .map(|row| row[key].as_str().expect("string field").to_string())
        .collect()
}

const SHELL_CONTROL: [&str; 5] = ["unset", "exit", "jobs", "fg", "bg"];

#[test]
fn help_and_schemas_list_pipeline_commands_and_leave_out_shell_control() {
    let dir = scratch_dir("catalog");

    let help: serde_json::Value =
        serde_json::from_str(&koa_stdout(&dir, "help | select command | to json"))
            .expect("help renders as json");
    let helped = json_strings(&help, "command");

    let mut rpc = RpcClient::start(&dir);
    rpc.send(serde_json::json!({"id": 1, "method": "session.schemas"}));
    let line = rpc
        .lines
        .recv_timeout(Duration::from_secs(5))
        .expect("session.schemas response");
    let response: serde_json::Value = serde_json::from_str(&line).expect("schemas is json");
    let schemas = json_strings(&response["result"]["schemas"], "name");

    for (source, names) in [("help", &helped), ("session.schemas", &schemas)] {
        for known in ["ls", "cat", "undo"] {
            assert!(
                names.iter().any(|n| n == known),
                "{source} lacks `{known}`: {names:?}"
            );
        }
        for hidden in SHELL_CONTROL {
            assert!(
                !names.iter().any(|n| n == hidden),
                "{source} lists shell-control command `{hidden}`: {names:?}"
            );
        }
    }
}

#[test]
fn undo_snapshots_live_on_disk_only_while_the_process_runs() {
    let dir = scratch_dir("undo-store-lifetime");
    std::fs::write(dir.join("f"), "original\n").expect("write f");
    let out = koa_c(
        &dir,
        "echo changed > f; ls .koa/undo | select name | to json",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let entries: serde_json::Value = serde_json::from_str(&out_text(&out))
        .unwrap_or_else(|e| panic!("ls output is JSON ({e}): {}", out_text(&out)));
    assert_eq!(
        entries.as_array().map(Vec::len),
        Some(1),
        "exactly one store directory must exist while the session runs: {entries}"
    );
    assert!(
        undo_store_sessions(&dir).is_empty(),
        "the store must be gone after exit: {:?}",
        undo_store_sessions(&dir)
    );
}

#[test]
fn the_next_koa_sweeps_the_undo_store_of_a_killed_one() {
    let dir = scratch_dir("undo-store-sweep");
    std::fs::write(dir.join("f"), "original\n").expect("write f");
    let mut doomed = Command::new(KOA)
        .arg("-c")
        .arg("echo changed > f; sleep 5")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn koa");
    let start = Instant::now();
    while std::fs::read_to_string(dir.join("f")).ok().as_deref() != Some("changed\n") {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "koa never performed the edit"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    doomed.kill().expect("kill koa");
    doomed.wait().expect("reap koa");
    assert!(
        !undo_store_sessions(&dir).is_empty(),
        "a killed koa leaves its store behind"
    );

    let out = koa_c(&dir, "true", Duration::from_secs(10)).expect("koa -c finishes");
    assert!(out.status.success());
    assert!(
        undo_store_sessions(&dir).is_empty(),
        "the next koa must sweep the stale store: {:?}",
        undo_store_sessions(&dir)
    );
}

#[test]
fn evicting_a_ledger_entry_deletes_its_snapshot() {
    let dir = scratch_dir("undo-store-eviction");
    std::fs::write(dir.join("f"), "original\n").expect("write f");
    let edits: String = (1..=60).map(|i| format!("echo {i} > f; ")).collect();
    let out = koa_c(
        &dir,
        &format!("{edits}find .koa/undo -type f | select path; undo; cat f"),
        Duration::from_secs(20),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let snapshots = out_text(&out).matches("snap-").count();
    assert!(
        (49..=50).contains(&snapshots),
        "60 edits keep the snapshots of the last 49 or 50 ledger entries, found {snapshots}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("f")).expect("f exists"),
        "59\n",
        "the surviving snapshots must be the newest ones, so undo reverts only the last edit"
    );
}

#[test]
fn undo_refuses_after_an_external_ran_with_nothing_recorded_before_it() {
    let dir = scratch_dir("undo-opaque-floor");
    std::fs::write(dir.join("f"), "original\n").expect("write f");
    let out = koa_c(
        &dir,
        "/bin/cp f g; undo; echo st=$?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E_TX_OPAQUE") && stderr.contains("/bin/cp"),
        "undo must refuse and name the command, stderr: {stderr}"
    );
    assert!(
        out_text(&out).contains("st=1"),
        "stdout: {}",
        out_text(&out)
    );
    assert!(dir.join("g").exists(), "a refused undo must not touch g");
}

#[test]
fn a_capturing_command_clears_the_refusal_left_by_an_earlier_external() {
    let dir = scratch_dir("undo-opaque-floor-cleared");
    std::fs::write(dir.join("f"), "original\n").expect("write f");
    let out = koa_c(
        &dir,
        "/bin/cp f g; echo new > f; undo; echo st=$?; cat f; undo; echo st2=$?",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("f")).expect("f exists"),
        "original\n"
    );
    let stdout = out_text(&out);
    assert!(stdout.contains("st=0"), "stdout: {stdout}");
    assert!(
        stdout.contains("st2=0"),
        "a later undo on the emptied ledger must not refuse, stdout: {stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("E_TX_OPAQUE"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn redo_brings_back_a_tree_that_rollback_deleted_even_when_an_earlier_record_names_its_root() {
    let dir = scratch_dir("redo-tree");
    std::fs::create_dir_all(dir.join("d")).expect("create d");
    std::fs::write(dir.join("d/c"), "child\n").expect("write c");
    let out = koa_c(
        &dir,
        "rm -r d; mkdir d; /bin/sh -c 'echo ext > d/x'; rollback tx_001; redo",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("d/x")).expect("d/x must be back after redo"),
        "ext\n"
    );
    assert!(
        !dir.join("d/c").exists(),
        "redo must restore the pre-rollback state"
    );
}

#[test]
fn startup_sweep_never_follows_a_symlinked_undo_dir() {
    let dir = scratch_dir("sweep-symlink");
    let victim = dir.join("victim");
    std::fs::create_dir_all(victim.join("old_work")).expect("victim dir");
    std::fs::write(victim.join("old_work/keep.txt"), "user data\n").expect("victim file");
    let aged = Command::new("touch")
        .args(["-d", "2 hours ago"])
        .arg(victim.join("old_work"))
        .status()
        .expect("touch");
    assert!(aged.success());
    std::fs::create_dir_all(dir.join(".koa")).expect(".koa");
    std::os::unix::fs::symlink("../victim", dir.join(".koa/undo")).expect("symlink");

    let out = koa_c(&dir, "echo hi", Duration::from_secs(10)).expect("koa -c finishes");
    assert!(out.status.success());
    assert!(
        victim.join("old_work/keep.txt").exists(),
        "the sweep must not delete through a symlinked .koa/undo"
    );
}

#[test]
fn undo_snapshots_are_invisible_to_git() {
    let dir = scratch_dir("undo-gitignore");
    let init = Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&dir)
        .status();
    if !init.is_ok_and(|s| s.success()) {
        return;
    }
    std::fs::write(dir.join("f"), "secret\n").expect("seed");
    let out = koa_c(
        &dir,
        "echo changed > f; /usr/bin/git status --porcelain --untracked-files=all",
        Duration::from_secs(10),
    )
    .expect("koa -c finishes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("f"), "git sees the user's file: {stdout}");
    assert!(
        !stdout.contains(".koa"),
        "undo snapshots must not show up as untracked files: {stdout}"
    );
}

#[test]
fn underscore_spellings_are_accepted_for_hyphenated_commands() {
    let fixture = |name: &str| {
        let dir = scratch_dir(name);
        std::fs::write(dir.join("a"), "").expect("write a");
        std::fs::write(dir.join("b"), "").expect("write b");
        dir
    };

    let hyphen = koa_stdout(
        &fixture("alias-sort-hyphen"),
        "ls | sort-by --desc name | select name | to json",
    );
    let underscore = koa_stdout(
        &fixture("alias-sort-underscore"),
        "ls | sort_by --desc name | select name | to json",
    );
    assert_eq!(underscore, hyphen);
    let rows: serde_json::Value = serde_json::from_str(&underscore).expect("json rows");
    assert_eq!(json_strings(&rows, "name"), ["b", "a"]);

    let hyphen = koa_stdout(
        &fixture("alias-log-hyphen"),
        "rm a; tx-log | where command == rm | to json",
    );
    let underscore = koa_stdout(
        &fixture("alias-log-underscore"),
        "rm a; tx_log | where command == rm | to json",
    );
    assert_eq!(underscore, hyphen);
    let rows: serde_json::Value = serde_json::from_str(&underscore).expect("json rows");
    assert_eq!(json_strings(&rows, "tx_id"), ["tx_001"]);
}

fn run_koa(dir: &Path, script: &str) -> (String, String, Option<i32>) {
    let out = koa_c(dir, script, Duration::from_secs(10)).expect("koa -c finishes");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

fn write_sized_files(dir: &Path) {
    std::fs::write(dir.join("a.txt"), "123").expect("write a.txt");
    std::fs::write(dir.join("b.txt"), "1234567890").expect("write b.txt");
}

fn json_rows(dir: &Path, script: &str) -> serde_json::Value {
    let (stdout, stderr, code) = run_koa(dir, script);
    assert_eq!(code, Some(0), "{script}: stderr {stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{script}: {e}: {stdout}"))
}

#[test]
fn where_supports_every_comparison_operator() {
    let dir = scratch_dir("where-ops");
    write_sized_files(&dir);
    let names = |script: &str| -> Vec<String> {
        json_rows(&dir, &format!("{script} | select name | to json"))
            .as_array()
            .expect("array")
            .iter()
            .map(|row| row["name"].as_str().expect("name").to_string())
            .collect()
    };
    let none: [&str; 0] = [];
    assert_eq!(names("ls | where size >= 10"), ["b.txt"]);
    assert_eq!(names("ls | where size >=10"), ["b.txt"]);
    assert_eq!(names("ls | where size > 10"), none);
    assert_eq!(names("ls | where size > 3"), ["b.txt"]);
    assert_eq!(names("ls | where size <= 3"), ["a.txt"]);
    assert_eq!(names("ls | where size < 3"), none);
    assert_eq!(names("ls | where size < 10"), ["a.txt"]);
    assert_eq!(names("ls | where size == 3"), ["a.txt"]);
    assert_eq!(names("ls | where size != 3"), ["b.txt"]);
    assert_eq!(names("ls | where name != a.txt"), ["b.txt"]);
    assert_eq!(names("ls | where name =~ b."), ["b.txt"]);
}

#[test]
fn where_rejects_unknown_operators_and_malformed_expressions() {
    let dir = scratch_dir("where-bad");
    write_sized_files(&dir);
    for script in [
        "ls | where size ~~ 5",
        "ls | where size >> 5",
        "ls | where size",
        "ls | where size > 5 extra",
    ] {
        let (stdout, stderr, code) = run_koa(&dir, script);
        assert_eq!(code, Some(2), "{script}: stdout {stdout} stderr {stderr}");
        assert!(stderr.contains("E_ARGS"), "{script}: stderr {stderr}");
    }
}

#[test]
fn redirects_still_work_around_where_syntax() {
    let dir = scratch_dir("redirects");
    write_sized_files(&dir);
    let (_, stderr, code) = run_koa(&dir, "echo hi > f; cat f");
    assert_eq!(code, Some(0), "stderr {stderr}");
    assert_eq!(std::fs::read_to_string(dir.join("f")).expect("f"), "hi\n");

    let (stdout, _, _) = run_koa(&dir, "echo more >> f; /bin/cat f");
    assert_eq!(stdout, "hi\nmore\n");

    let (stdout, _, _) = run_koa(&dir, "/bin/sh -c 'echo e >&2' 2>&1 | /bin/cat");
    assert_eq!(stdout, "e\n");

    let (_, stderr, code) = run_koa(&dir, "ls | where size >= 5 > out.txt");
    assert_eq!(code, Some(0), "stderr {stderr}");
    assert!(
        std::fs::read_to_string(dir.join("out.txt"))
            .expect("out.txt")
            .contains("b.txt")
    );
}

#[test]
fn sql_keeps_the_quotes_the_user_wrote() {
    let dir = scratch_dir("sql-quotes");
    assert_eq!(
        json_rows(&dir, "@sql SELECT 'a' AS v | to json"),
        serde_json::json!([{"v": "a"}])
    );
    assert_eq!(
        json_rows(&dir, r#"@sql SELECT "x" AS y | to json"#),
        serde_json::json!([{"y": "x"}])
    );
    assert_eq!(
        json_rows(&dir, "@sql SELECT 5 AS n WHERE n > 1 | to json"),
        serde_json::json!([{"n": 5}])
    );
    assert!(
        !dir.join("1").exists(),
        "`> 1` inside SQL is not a redirect"
    );
    assert_eq!(
        json_rows(&dir, "@sql \"SELECT 'q' AS w\" | to json"),
        serde_json::json!([{"w": "q"}])
    );
}

#[test]
fn sql_concatenation_operator_is_part_of_the_query() {
    let dir = scratch_dir("sql-concat");
    for script in [
        "@sql SELECT 'a' || 'b' AS v | to json",
        "@sql SELECT 'a' ||'b' AS v | to json",
        "@sql SELECT 'a'|| 'b' AS v | to json",
    ] {
        assert_eq!(
            json_rows(&dir, script),
            serde_json::json!([{"v": "ab"}]),
            "{script}"
        );
    }
}

#[test]
fn or_is_source_after_js_and_a_shell_chain_after_py() {
    if !have("python3") || !have("node") {
        return;
    }
    let dir = scratch_dir("sigil-or");
    let (stdout, _, code) = run_koa(&dir, "@py import no_such_module_zz || echo recovered");
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "recovered\n");

    let (stdout, _, code) = run_koa(&dir, "@js throw 0 || new Error");
    assert_eq!(code, Some(1), "`new Error` must not run as a shell command");
    assert_eq!(stdout, "");
}

#[test]
fn py_and_js_take_one_quoted_program_as_before() {
    if !have("python3") || !have("node") {
        return;
    }
    let dir = scratch_dir("py-js-quotes");
    let (stdout, stderr, code) = run_koa(&dir, r#"@py 'print("a b")'"#);
    assert_eq!(code, Some(0), "stderr {stderr}");
    assert_eq!(stdout, "a b\n");

    let (stdout, stderr, code) = run_koa(&dir, r#"@js 'console.log("x")'"#);
    assert_eq!(code, Some(0), "stderr {stderr}");
    assert_eq!(stdout, "x\n");

    let (stdout, stderr, code) = run_koa(&dir, "@py 'print(1)'; @js 'console.log(2)'");
    assert_eq!(code, Some(0), "stderr {stderr}");
    assert_eq!(stdout, "1\n2\n");
}

#[test]
fn hashline_builtins_print_the_columns_help_lists_for_them() {
    let dir = scratch_dir("hashline-columns");
    std::fs::write(dir.join("a.txt"), "alpha\nbeta\n").expect("write a.txt");
    let run = |script: &str| {
        let out = koa_c(&dir, script, Duration::from_secs(10)).expect("koa -c finishes");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    for (name, script) in [
        ("cat", "cat a.txt"),
        ("head", "head -n 1 a.txt"),
        ("tail", "tail -n 1 a.txt"),
        ("grep", "grep alpha a.txt"),
        ("refs", "refs alpha"),
    ] {
        let compact = run(&format!("{script} | to compact"));
        let header = compact.lines().next().unwrap_or_default();
        let printed = header
            .strip_prefix("@schema:")
            .unwrap_or_else(|| panic!("`{script}` printed no table: {compact:?}"))
            .replace('|', ", ");
        let listed: serde_json::Value =
            serde_json::from_str(&run(&format!("help {name} | to json")))
                .expect("help prints JSON");
        let row = listed
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["command"] == name))
            .unwrap_or_else(|| panic!("help lists no `{name}`: {listed}"));
        assert_eq!(row["columns"], printed, "{name}");
    }
}

#[test]
fn rpc_timeout_stops_an_in_process_sql_query() {
    let dir = scratch_dir("rpc-sql-timeout");
    let mut rpc = RpcClient::start(&dir);
    let sid = rpc.create_session();
    let endless = "@sql 'WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c) SELECT count(*) FROM c'";
    let lines = exec_lines(&mut rpc, &sid, 2, endless, Some(200));
    let completed: serde_json::Value = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["event"] == "completed" || v.get("error").is_some())
        .expect("exec must finish with a completed event or an error");
    assert_eq!(completed["exit_code"], 124, "{completed}");
    let next = exec_lines(&mut rpc, &sid, 3, "echo next", None);
    assert_eq!(
        stdout_of(&next),
        "next\n",
        "the session must accept the next command once the query is stopped: {next:?}"
    );
    let after = exec_lines(
        &mut rpc,
        &sid,
        4,
        "@sql 'SELECT 41 + 1 AS v' | to json",
        None,
    );
    let rows: serde_json::Value =
        serde_json::from_str(&stdout_of(&after)).expect("to json prints JSON");
    assert_eq!(rows, serde_json::json!([{"v": 42}]), "{after:?}");
}
