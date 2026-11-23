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
fn table_verbs_count_skip_reverse_reject() {
    let dir = scratch("basic");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(dir.join(name), name).expect("write");
    }

    let rows = json_rows(&koa(&dir, "ls | count | to json"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["count"], 4);

    let rows = json_rows(&koa(&dir, "ls | length | to json"));
    assert_eq!(rows[0]["count"], 4);

    let rows = json_rows(&koa(
        &dir,
        "ls | sort-by name | skip 2 | select name | to json",
    ));
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["c.txt", "d.txt"]);

    let rows = json_rows(&koa(
        &dir,
        "ls | sort-by name | reverse | first 2 | select name | to json",
    ));
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["d.txt", "c.txt"]);

    let rows = json_rows(&koa(
        &dir,
        "ls | reject mode mtime type size | first 1 | to json",
    ));
    let obj = rows[0].as_object().unwrap();
    assert!(obj.contains_key("name"));
    assert!(!obj.contains_key("size"));
    assert!(!obj.contains_key("mode"));
}

#[test]
fn table_verbs_uniq_group_by_schema_stats() {
    let dir = scratch("agg");
    fs::write(
        dir.join("data.json"),
        r#"[{"lang":"rust","lines":100,"files":2},{"lang":"go","lines":50,"files":1},{"lang":"rust","lines":200,"files":3},{"lang":"py","lines":25,"files":1}]"#,
    )
    .expect("write json");

    let rows = json_rows(&koa(
        &dir,
        "cat --raw data.json | from json | group-by lang | to json",
    ));
    assert_eq!(rows[0]["group"], "rust");
    assert_eq!(rows[0]["count"], 2);
    assert_eq!(rows.len(), 3);

    let rows = json_rows(&koa(
        &dir,
        "cat --raw data.json | from json | group-by --asc lang | first 1 | to json",
    ));
    assert_eq!(rows[0]["count"], 1);

    let rows = json_rows(&koa(
        &dir,
        "cat --raw data.json | from json | uniq lang | count | to json",
    ));
    assert_eq!(rows[0]["count"], 3);

    let rows = json_rows(&koa(
        &dir,
        "cat --raw data.json | from json | uniq -c lang | where lang == rust | to json",
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["count"], 2);

    let rows = json_rows(&koa(
        &dir,
        "cat --raw data.json | from json | schema | to json",
    ));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["column"], "lang");
    assert_eq!(rows[1]["kind"], "str");
    assert_eq!(rows[2]["column"], "lines");
    assert_eq!(rows[2]["kind"], "uint");
    assert_eq!(rows[2]["non_null"], 4);

    let rows = json_rows(&koa(
        &dir,
        "cat --raw data.json | from json | stats lines | to json",
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["column"], "lines");
    assert_eq!(rows[0]["count"], 4);
    assert_eq!(rows[0]["sum"], 375);
    assert_eq!(rows[0]["min"], 25);
    assert_eq!(rows[0]["max"], 200);
    assert_eq!(rows[0]["avg"], 93.75);
}

#[test]
fn tee_preserves_table_and_supports_undo() {
    let dir = scratch("tee");
    fs::write(dir.join("alpha.txt"), "1").expect("write");
    fs::write(dir.join("beta.txt"), "22").expect("write");

    let rows = json_rows(&koa(
        &dir,
        "ls | sort-by name | tee names.txt | where name == beta.txt | count | to json",
    ));
    assert_eq!(rows[0]["count"], 1);
    let saved = fs::read_to_string(dir.join("names.txt")).expect("read tee file");
    assert_eq!(saved, "alpha.txt\nbeta.txt\n");

    let out = koa(&dir, "ls | tee out.txt | count; undo");
    assert!(out.status.success());
    assert!(!dir.join("out.txt").exists());
}
