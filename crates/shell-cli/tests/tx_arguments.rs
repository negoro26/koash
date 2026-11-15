#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

const KOA: &str = env!("CARGO_BIN_EXE_koa");

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("koa-txargs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn run(&self, script: &str) -> (String, String) {
        let out = Command::new(KOA)
            .arg("-c")
            .arg(script)
            .current_dir(&self.0)
            .output()
            .expect("run koa");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn undo_and_redo_reject_any_argument_and_change_nothing() {
    for (case, script) in [
        (
            "undo-operand",
            "echo a > f; echo b > f; undo extra; echo st=$?; cat --raw f",
        ),
        (
            "undo-option",
            "echo a > f; echo b > f; undo -x; echo st=$?; cat --raw f",
        ),
        (
            "redo-operand",
            "echo a > f; echo b > f; undo > /dev/null; redo extra; echo st=$?; cat --raw f",
        ),
    ] {
        let s = Scratch::new(case);
        let (stdout, stderr) = s.run(script);
        let expected = if case.starts_with("redo") {
            "st=2\na\n"
        } else {
            "st=2\nb\n"
        };
        assert_eq!(stdout, expected, "{case}: {stderr}");
        assert!(stderr.contains("E_ARGS"), "{case}: {stderr}");
    }

    let s = Scratch::new("undo-redo-plain");
    let (stdout, stderr) = s.run(
        "echo a > f; echo b > f; undo > /dev/null; cat --raw f; redo > /dev/null; cat --raw f",
    );
    assert_eq!(stdout, "a\nb\n", "{stderr}");
}

#[test]
fn tx_log_rejects_any_argument() {
    let s = Scratch::new("txlog");
    let (stdout, stderr) = s.run("tx-log extra; echo st=$?; tx-log | first 0; echo st=$?");
    assert_eq!(stdout, "st=2\nst=0\n", "{stderr}");
    assert!(stderr.contains("E_ARGS"), "{stderr}");
}

#[test]
fn rollback_takes_exactly_one_transaction_id() {
    for (case, script) in [
        (
            "none",
            "echo a > f; echo b > f; rollback; echo st=$?; cat --raw f",
        ),
        (
            "two",
            "echo a > f; echo b > f; rollback tx_002 extra; echo st=$?; cat --raw f",
        ),
    ] {
        let s = Scratch::new(case);
        let (stdout, stderr) = s.run(script);
        assert_eq!(stdout, "st=2\nb\n", "{case}: {stderr}");
        assert!(stderr.contains("E_ARGS"), "{case}: {stderr}");
    }

    let s = Scratch::new("rollback-one");
    let (stdout, stderr) =
        s.run("echo a > f; echo b > f; rollback tx_002 > /dev/null; cat --raw f");
    assert_eq!(stdout, "a\n", "{stderr}");
}
