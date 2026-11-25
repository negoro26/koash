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
fn set_errexit_pipefail_and_pipestatus() {
    let dir = scratch("set");

    let rows = json_rows(&koa(&dir, "set -o | to json"));
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["option"], "errexit");
    assert_eq!(rows[0]["enabled"], false);
    assert_eq!(rows[1]["option"], "noglob");
    assert_eq!(rows[1]["enabled"], false);
    assert_eq!(rows[2]["option"], "nounset");
    assert_eq!(rows[2]["enabled"], false);
    assert_eq!(rows[3]["option"], "pipefail");
    assert_eq!(rows[3]["enabled"], false);

    let out = koa(&dir, "set -e; cat nope; echo should-not-run");
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("should-not-run"));

    let out = koa(&dir, "set -e; cat nope || echo recovered; echo after");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("recovered"));
    assert!(stdout.contains("after"));

    let out = koa(&dir, "cat nope | first 1; echo $PIPESTATUS");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 0"));

    let out = koa(&dir, "set -o pipefail; cat nope | first 1");
    assert!(!out.status.success());

    let out = koa(
        &dir,
        "sh -c 'exit 3' | sh -c 'exit 2' | true; echo $PIPESTATUS",
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "3 2 0");

    let out = koa(&dir, "set -o pipefail; sh -c 'exit 3' | true");
    assert_eq!(out.status.code(), Some(3));

    let out = koa(&dir, "set -eo pipefail; true | false; echo should-not-run");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}

#[test]
fn negation_and_noglob() {
    let dir = scratch("negation");
    fs::write(dir.join("a.txt"), "").expect("write");
    fs::write(dir.join("b.txt"), "").expect("write");

    let out = koa(&dir, "set -e; ! cat nope; echo survived");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("survived"));

    let out = koa(&dir, "! echo ok");
    assert_eq!(out.status.code(), Some(1));

    let out = koa(&dir, "set -f; echo *.txt");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "*.txt");
}
