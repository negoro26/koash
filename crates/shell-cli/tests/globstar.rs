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

#[test]
fn globstar_recursive_and_cycle_safe() {
    let dir = scratch("globstar");
    fs::create_dir_all(dir.join("a/b/c")).expect("mkdir");
    fs::create_dir_all(dir.join(".hidden")).expect("mkdir hidden");
    fs::write(dir.join("root.rs"), "").expect("write");
    fs::write(dir.join("a/one.rs"), "").expect("write");
    fs::write(dir.join("a/b/c/deep.rs"), "").expect("write");
    fs::write(dir.join(".hidden/secret.rs"), "").expect("write");
    std::os::unix::fs::symlink(dir.join("a"), dir.join("a/b/loop")).expect("symlink");

    let out = koa(&dir, "echo **/*.rs");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut got: Vec<&str> = stdout.split_whitespace().collect();
    got.sort_unstable();
    assert_eq!(got, vec!["a/b/c/deep.rs", "a/one.rs", "root.rs"]);
}
