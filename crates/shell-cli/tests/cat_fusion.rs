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
fn cat_then_first_or_last_keeps_line_numbers() {
    let dir = scratch("catslice");
    fs::write(dir.join("lines.txt"), "one\ntwo\nthree\nfour\nfive\n").expect("write");
    let rows = json_rows(&koa(&dir, "cat lines.txt | last 2 | to json"));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["line"], 4);
    assert_eq!(rows[0]["text"], "four");
    assert_eq!(rows[1]["line"], 5);
    assert_eq!(rows[1]["text"], "five");

    let rows = json_rows(&koa(&dir, "cat lines.txt | first 2 | to json"));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["line"], 1);
    assert_eq!(rows[0]["text"], "one");
}
