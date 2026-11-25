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
fn from_to_csv_tsv_lines_md() {
    let dir = scratch("formats");
    fs::write(
        dir.join("quoted.csv"),
        "name,note\n\"a,b\",\"say \"\"hi\"\"\"\nplain,ok\n",
    )
    .expect("write csv");

    let rows = json_rows(&koa(&dir, "cat --raw quoted.csv | from csv | to json"));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "a,b");
    assert_eq!(rows[0]["note"], "say \"hi\"");

    let out = koa(&dir, "cat --raw quoted.csv | from csv | to csv");
    assert!(out.status.success());
    let roundtrip = json_rows(&koa(
        &dir,
        "cat --raw quoted.csv | from csv | to csv | from csv | to json",
    ));
    assert_eq!(rows, roundtrip);

    let roundtrip_tsv = json_rows(&koa(
        &dir,
        "cat --raw quoted.csv | from csv | to tsv | from tsv | to json",
    ));
    assert_eq!(rows, roundtrip_tsv);

    let md = koa(&dir, "cat --raw quoted.csv | from csv | to md");
    assert!(md.status.success());
    let md_text = String::from_utf8_lossy(&md.stdout);
    assert!(md_text.contains("| name | note |"));
    assert!(md_text.contains("| --- | --- |"));
    assert!(md_text.contains("| a,b | say \"hi\" |"));

    let lines = json_rows(&koa(&dir, "echo 'one\ntwo\nthree' | from lines | to json"));
    let texts: Vec<&str> = lines.iter().map(|r| r["line"].as_str().unwrap()).collect();
    assert_eq!(texts, vec!["one", "two", "three"]);
}
