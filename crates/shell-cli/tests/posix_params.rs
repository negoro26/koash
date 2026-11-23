#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const KOA: &str = env!("CARGO_BIN_EXE_koa");

struct ScratchDir(PathBuf);

impl std::ops::Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> ScratchDir {
    let dir = std::env::temp_dir().join(format!("koa-verbs-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    ScratchDir(dir)
}

fn koa(cwd: &Path, script: &str) -> Output {
    Command::new(KOA)
        .args(["-c", script])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run koa")
}

fn json_rows(out: &Output) -> Vec<serde_json::Value> {
    assert!(
        out.status.success(),
        "koa failed ({:?}): stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("valid json array")
}

#[test]
fn pipe_both_and_posix_param_expansions() {
    let dir = scratch("paramexp");

    let out = koa(
        &dir,
        "sh -c 'echo out; echo err >&2' |& sh -c 'grep err' | to json",
    );
    let rows = json_rows(&out);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["line"], "err");

    let out = koa(
        &dir,
        "FOO=hello; EMPTY=''; echo ${#FOO} ${UNSET:-fallback} ${EMPTY:-empty_fb} ${EMPTY-set_empty} ${FOO:+present} ${UNSET:+nope}",
    );
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.trim(), "5 fallback empty_fb present");
}

#[test]
fn ansi_c_quotes() {
    let dir = scratch("ansic");

    let out = koa(&dir, r"echo -n $'a\tb\n\x41\102\u0043'");
    assert!(out.status.success());
    assert_eq!(out.stdout, b"a\tb\nABC");

    let out = koa(&dir, r#"echo -n "$'no\nesc'""#);
    assert!(out.status.success());
    assert_eq!(out.stdout, b"$'no\\nesc'");

    let out = koa(&dir, r"echo -n $'\cA\c?'");
    assert!(out.status.success());
    assert_eq!(out.stdout, b"\x01\x7f");
}
