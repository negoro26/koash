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
fn dirstack_and_type_builtin() {
    let dir = scratch("dirstack");
    fs::create_dir(dir.join("sub1")).expect("mkdir");
    fs::create_dir(dir.join("sub2")).expect("mkdir");

    let rows = json_rows(&koa(
        &dir,
        "pushd sub1 > /dev/null; pushd ../sub2 > /dev/null; dirs | to json",
    ));
    assert_eq!(rows.len(), 3);
    assert!(rows[0]["path"].as_str().unwrap().ends_with("sub2"));
    assert!(rows[1]["path"].as_str().unwrap().ends_with("sub1"));

    let out = koa(&dir, "pushd sub1; popd; pwd");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains(&dir.display().to_string()));

    let rows = json_rows(&koa(&dir, "alias ll='ls -la'; type ll ls sh | to json"));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["kind"], "alias");
    assert_eq!(rows[0]["detail"], "ls -la");
    assert_eq!(rows[1]["kind"], "builtin");
    assert_eq!(rows[2]["kind"], "external");
}
