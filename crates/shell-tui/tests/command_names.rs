#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

use shell_tui::complete::complete_line;
use shell_tui::highlight_line;

const TEST_NAME: &str = "builtin_commands_complete_and_highlight_as_commands";
const CHILD_MARKER: &str = "KOA_TUI_TEST_EMPTY_PATH_CHILD";

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// A host that ships `/usr/bin/jobs` would pass without the builtin registry, so the assertions run with an empty `PATH`.
#[test]
fn builtin_commands_complete_and_highlight_as_commands() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        let empty = std::env::var_os("PATH").expect("PATH set by the parent test");
        assertions(Path::new(&empty));
        return;
    }

    let empty = std::env::temp_dir().join(format!("koa-tui-empty-path-{}", std::process::id()));
    std::fs::create_dir_all(&empty).expect("create empty PATH dir");

    let out = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_MARKER, "1")
        .env("PATH", &empty)
        .output()
        .expect("run child test");
    let _ = std::fs::remove_dir_all(&empty);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "child run failed\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assertions(cwd: &Path) {
    let unset = complete_line("unse", 4, cwd);
    assert!(unset.contains(&"unset".to_string()), "got {unset:?}");
    let jobs = complete_line("jo", 2, cwd);
    assert!(jobs.contains(&"jobs".to_string()), "got {jobs:?}");
    let ls = complete_line("l", 1, cwd);
    assert!(ls.contains(&"ls".to_string()), "got {ls:?}");
    let nothing = complete_line("zzqq-no-such", 12, cwd);
    assert!(nothing.is_empty(), "got {nothing:?}");

    assert_eq!(highlight_line("jobs"), format!("{GREEN}jobs{RESET}"));
    assert_eq!(
        highlight_line("unset FOO"),
        format!("{GREEN}unset{RESET} FOO")
    );
    assert_eq!(highlight_line("ls"), format!("{GREEN}ls{RESET}"));
    assert_eq!(
        highlight_line("zzqq-no-such"),
        format!("{RED}zzqq-no-such{RESET}")
    );
}
