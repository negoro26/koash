#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const KOA: &str = env!("CARGO_BIN_EXE_koa");

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("koa-fs-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir.canonicalize().expect("canonical scratch dir"))
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn q(&self, rel: &str) -> String {
        format!("'{}'", self.path(rel).display())
    }

    fn write(&self, rel: &str, content: &[u8]) -> PathBuf {
        let p = self.path(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("parent dir");
        }
        fs::write(&p, content).expect("write fixture");
        p
    }

    fn mkdir(&self, rel: &str) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(&p).expect("mkdir fixture");
        p
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).expect("read fixture")
    }

    fn exists(&self, rel: &str) -> bool {
        fs::symlink_metadata(self.path(rel)).is_ok()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

struct Koa<'a> {
    cwd: &'a Path,
    script: String,
    stdin: Option<Vec<u8>>,
    envs: Vec<(String, String)>,
    removed: Vec<&'static str>,
    umask: Option<&'static str>,
}

fn koa<'a>(cwd: &'a Path, script: impl Into<String>) -> Koa<'a> {
    Koa {
        cwd,
        script: script.into(),
        stdin: None,
        envs: Vec::new(),
        removed: vec!["CDPATH"],
        umask: None,
    }
}

impl Koa<'_> {
    fn stdin(mut self, bytes: &[u8]) -> Self {
        self.stdin = Some(bytes.to_vec());
        self
    }

    fn env(mut self, key: &str, value: impl AsRef<str>) -> Self {
        self.envs
            .push((key.to_string(), value.as_ref().to_string()));
        self
    }

    fn without_env(mut self, key: &'static str) -> Self {
        self.removed.push(key);
        self
    }

    fn umask(mut self, mask: &'static str) -> Self {
        self.umask = Some(mask);
        self
    }

    fn run(self) -> Run {
        let mut cmd = match self.umask {
            Some(mask) => {
                let mut cmd = Command::new("/bin/sh");
                cmd.args(["-c", "umask \"$1\"; shift; exec \"$@\"", "sh", mask, KOA]);
                cmd
            }
            None => Command::new(KOA),
        };
        cmd.arg("-c")
            .arg(&self.script)
            .current_dir(self.cwd)
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in &self.removed {
            cmd.env_remove(key);
        }
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn koa");
        if let Some(bytes) = &self.stdin {
            let mut pipe = child.stdin.take().expect("stdin pipe");
            pipe.write_all(bytes).expect("feed stdin");
        }
        let out = child.wait_with_output().expect("collect output");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

fn run(cwd: &Path, script: &str) -> Run {
    koa(cwd, script).run()
}

fn run_builtins_only(s: &Scratch, script: &str) -> Run {
    let empty = s.mkdir("no-programs");
    koa(&s.0, script).env("PATH", empty.to_string_lossy()).run()
}

#[test]
fn rm_force_long_option_keeps_a_directory_without_recursive() {
    let s = Scratch::new("rm-force-dir");
    s.write("d/inner", b"keep");
    let r = run(&s.0, &format!("rm --force {}", s.q("d")));
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("Is a directory"), "{}", r.stderr);
    assert_eq!(s.read("d/inner"), "keep");
}

#[test]
fn rm_verbose_long_option_does_not_imply_recursive() {
    let s = Scratch::new("rm-verbose-dir");
    s.write("d/inner", b"keep");
    let r = run(&s.0, &format!("rm --verbose {}", s.q("d")));
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert_eq!(s.read("d/inner"), "keep");
}

#[test]
fn rm_plain_directory_fails_and_keeps_it() {
    let s = Scratch::new("rm-plain-dir");
    s.write("d/inner", b"keep");
    let r = run(&s.0, &format!("rm {}", s.q("d")));
    assert_eq!(r.code, 1);
    assert_eq!(s.read("d/inner"), "keep");
}

#[test]
fn rm_recursive_removes_the_tree_and_dir_flag_only_empty_directories() {
    let s = Scratch::new("rm-recursive");
    s.write("tree/a/b/f", b"x");
    s.mkdir("empty");
    s.write("full/f", b"x");
    let r = run(
        &s.0,
        &format!("rm -r {}; rm -d {}", s.q("tree"), s.q("empty")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!s.exists("tree") && !s.exists("empty"));
    let r = run(&s.0, &format!("rm -d {}", s.q("full")));
    assert_eq!(r.code, 1);
    assert!(s.exists("full/f"));
}

#[test]
fn rm_unlinks_symlinks_without_touching_their_targets() {
    let s = Scratch::new("rm-symlinks");
    s.write("target/f", b"keep");
    symlink(s.path("target"), s.path("to_dir")).unwrap();
    symlink(s.path("nowhere"), s.path("dangling")).unwrap();
    let r = run(&s.0, &format!("rm {} {}", s.q("to_dir"), s.q("dangling")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!s.exists("to_dir") && !s.exists("dangling"));
    assert_eq!(s.read("target/f"), "keep");
}

#[test]
fn rm_recursive_does_not_follow_symlinks_out_of_the_tree() {
    let s = Scratch::new("rm-recursive-link");
    s.write("outside/f", b"keep");
    s.mkdir("tree");
    symlink(s.path("outside"), s.path("tree/link")).unwrap();
    let r = run(&s.0, &format!("rm -r {}", s.q("tree")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!s.exists("tree"));
    assert_eq!(s.read("outside/f"), "keep");
}

#[test]
fn rm_attempts_every_operand_and_exits_1_when_one_fails() {
    let s = Scratch::new("rm-accumulate");
    s.write("a", b"x");
    s.write("b", b"x");
    let r = run(
        &s.0,
        &format!("rm {} {} {}", s.q("a"), s.q("missing"), s.q("b")),
    );
    assert_eq!(r.code, 1);
    assert!(!s.exists("a") && !s.exists("b"));
    assert_eq!(r.stderr.matches("koa:").count(), 1, "{}", r.stderr);
    assert!(r.stderr.contains("missing"), "{}", r.stderr);
}

#[test]
fn rm_force_ignores_missing_operands_silently() {
    let s = Scratch::new("rm-force-missing");
    let r = run(&s.0, &format!("rm -f {}", s.q("missing")));
    assert_eq!(r.code, 0);
    assert_eq!(r.stderr, "");
}

#[test]
fn rm_unknown_option_is_a_usage_error_that_touches_nothing() {
    let s = Scratch::new("rm-unknown");
    s.write("a", b"x");
    let r = run_builtins_only(&s, &format!("rm --frobnicate {}", s.q("a")));
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--frobnicate"), "{}", r.stderr);
    assert!(s.exists("a"));
    let r = run_builtins_only(&s, &format!("rm -x {}", s.q("a")));
    assert_eq!(r.code, 2);
    assert!(s.exists("a"));
}

#[test]

fn rm_recursive_refuses_dot_and_dotdot() {
    let s = Scratch::new("rm-dot");
    s.write("work/sub/f", b"keep");
    s.write("work/f", b"keep");
    let work = s.path("work");
    for operand in [".", "..", "./", "sub/..", "sub/."] {
        let r = run(&work, &format!("rm -r {operand}"));
        assert_eq!(r.code, 1, "rm -r {operand}: {}", r.stderr);
        assert_eq!(s.read("work/sub/f"), "keep", "rm -r {operand}");
        assert_eq!(s.read("work/f"), "keep", "rm -r {operand}");
    }
}

#[test]
fn rm_recursive_removes_directories_whose_names_start_with_dots() {
    let s = Scratch::new("rm-dotnames");
    s.write("work/.hidden/f", b"x");
    s.write("work/..x/f", b"x");
    s.write("work/keep", b"keep");
    let r = run(&s.path("work"), "rm -r .hidden ..x");
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(!s.exists("work/.hidden") && !s.exists("work/..x"));
    assert_eq!(s.read("work/keep"), "keep");
}

#[test]
fn rm_verbose_names_each_removed_path() {
    let s = Scratch::new("rm-verbose");
    s.write("a", b"x");
    s.write("b", b"x");
    let r = run(&s.0, &format!("rm -v {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", r.stdout);
    assert!(
        lines[0].contains(&s.path("a").display().to_string()),
        "{}",
        r.stdout
    );
    assert!(
        lines[1].contains(&s.path("b").display().to_string()),
        "{}",
        r.stdout
    );
}

#[test]
fn cp_copies_every_source_into_the_target_directory() {
    let s = Scratch::new("cp-many");
    s.write("a", b"A");
    s.write("b", b"B");
    s.mkdir("dst");
    let r = run(
        &s.0,
        &format!("cp {} {} {}", s.q("a"), s.q("b"), s.q("dst")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("dst/a"), "A");
    assert_eq!(s.read("dst/b"), "B");
    assert_eq!(s.read("a"), "A");
    assert_eq!(s.read("b"), "B");
}

#[test]
fn cp_with_many_sources_needs_a_directory_target() {
    let s = Scratch::new("cp-many-file");
    s.write("a", b"A");
    s.write("b", b"B");
    s.write("c", b"C");
    let r = run(&s.0, &format!("cp {} {} {}", s.q("a"), s.q("b"), s.q("c")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("not a directory"), "{}", r.stderr);
    assert_eq!(s.read("b"), "B");
    assert_eq!(s.read("c"), "C");
}

#[test]
fn cp_recursive_into_an_existing_directory_nests_the_source() {
    let s = Scratch::new("cp-nest");
    s.write("src/f", b"F");
    s.mkdir("existing");
    let r = run(&s.0, &format!("cp -r {} {}", s.q("src"), s.q("existing")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("existing/src/f"), "F");
    assert!(!s.exists("existing/f"));
}

#[test]
fn cp_recursive_to_a_new_name_copies_the_tree() {
    let s = Scratch::new("cp-newname");
    s.write("src/a/f", b"F");
    let r = run(&s.0, &format!("cp -R {} {}", s.q("src"), s.q("copy")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("copy/a/f"), "F");
    let r = run(
        &s.0,
        &format!("cp --recursive {} {}", s.q("src"), s.q("copy2")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("copy2/a/f"), "F");
}

#[test]
fn cp_directory_without_recursive_is_omitted_with_status_1() {
    let s = Scratch::new("cp-omit");
    s.write("src/f", b"F");
    let r = run(&s.0, &format!("cp {} {}", s.q("src"), s.q("copy")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("-r not specified"), "{}", r.stderr);
    assert!(!s.exists("copy"));
}

#[test]
fn cp_no_clobber_keeps_the_existing_target() {
    let s = Scratch::new("cp-noclobber");
    s.write("a", b"new");
    s.write("b", b"old");
    let r = run(&s.0, &format!("cp -n {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "old");
    let r = run(&s.0, &format!("cp --no-clobber {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "old");
}

#[test]
fn cp_replaces_an_existing_target_by_default() {
    let s = Scratch::new("cp-replace");
    s.write("a", b"new");
    s.write("b", b"old");
    let r = run(&s.0, &format!("cp {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "new");
}

#[test]
fn cp_force_replaces_a_read_only_target() {
    let s = Scratch::new("cp-force");
    s.write("a", b"new");
    let b = s.write("b", b"old");
    fs::set_permissions(&b, fs::Permissions::from_mode(0o444)).unwrap();
    let r = run(&s.0, &format!("cp -f {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "new");
}

fn nix_is_root() -> bool {
    fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

#[test]
fn cp_preserve_keeps_mode_and_mtime() {
    let s = Scratch::new("cp-preserve");
    let a = s.write("a", b"A");
    fs::set_permissions(&a, fs::Permissions::from_mode(0o640)).unwrap();
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    let r = koa(&s.0, format!("cp -p {} {}", s.q("a"), s.q("b")))
        .umask("077")
        .run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    let meta = fs::metadata(s.path("b")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    assert_eq!(meta.modified().unwrap(), stamp);
}

/// The sources are empty because the kernel clears setuid on any write.
#[test]
fn cp_without_preserve_applies_the_umask_to_a_new_file() {
    let s = Scratch::new("cp-umask");
    for (mode, expected) in [(0o666, 0o644), (0o755, 0o755), (0o4755, 0o755)] {
        let a = s.write("a", b"");
        fs::set_permissions(&a, fs::Permissions::from_mode(mode)).unwrap();
        let r = koa(&s.0, format!("cp {} {}", s.q("a"), s.q("b")))
            .umask("022")
            .run();
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert_eq!(
            fs::metadata(s.path("b")).unwrap().permissions().mode() & 0o7777,
            expected,
            "source mode {mode:o} under umask 022"
        );
        fs::remove_file(s.path("a")).unwrap();
        fs::remove_file(s.path("b")).unwrap();
    }
    let a = s.write("a", b"A");
    fs::set_permissions(&a, fs::Permissions::from_mode(0o664)).unwrap();
    let r = koa(&s.0, format!("cp {} {}", s.q("a"), s.q("b")))
        .umask("027")
        .run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        fs::metadata(s.path("b")).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn cp_onto_an_existing_file_keeps_its_mode() {
    let s = Scratch::new("cp-existing-mode");
    let a = s.write("a", b"new");
    fs::set_permissions(&a, fs::Permissions::from_mode(0o644)).unwrap();
    let b = s.write("b", b"old");
    fs::set_permissions(&b, fs::Permissions::from_mode(0o600)).unwrap();
    let r = koa(&s.0, format!("cp {} {}", s.q("a"), s.q("b")))
        .umask("022")
        .run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "new");
    assert_eq!(
        fs::metadata(s.path("b")).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn cp_without_preserve_stamps_the_copy_with_the_current_time() {
    let s = Scratch::new("cp-nopreserve");
    let a = s.write("a", b"A");
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    let r = run(&s.0, &format!("cp {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_ne!(
        fs::metadata(s.path("b")).unwrap().modified().unwrap(),
        stamp
    );
}

#[test]
fn cp_recursive_copies_symlinks_as_symlinks() {
    let s = Scratch::new("cp-symlink");
    s.write("src/real", b"R");
    symlink("real", s.path("src/link")).unwrap();
    let r = run(&s.0, &format!("cp -r {} {}", s.q("src"), s.q("copy")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        fs::read_link(s.path("copy/link")).unwrap(),
        Path::new("real")
    );
}

#[test]
fn cp_dereference_follows_symlinks() {
    let s = Scratch::new("cp-deref");
    s.write("src/real", b"R");
    symlink("real", s.path("src/link")).unwrap();
    let r = run(&s.0, &format!("cp -rL {} {}", s.q("src"), s.q("copy")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let meta = fs::symlink_metadata(s.path("copy/link")).unwrap();
    assert!(meta.is_file(), "link should have been copied as a file");
    assert_eq!(s.read("copy/link"), "R");
}

#[test]
fn cp_refuses_to_copy_a_directory_into_itself() {
    let s = Scratch::new("cp-itself");
    s.write("d/f", b"F");
    let r = run(&s.0, &format!("cp -r {} {}", s.q("d"), s.q("d/sub")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("into itself"), "{}", r.stderr);
    assert!(!s.exists("d/sub"));
}

#[test]
fn cp_file_onto_itself_keeps_its_content() {
    let s = Scratch::new("cp-same");
    s.write("a", b"precious");
    let r = run(&s.0, &format!("cp {} {}", s.q("a"), s.q("a")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("same file"), "{}", r.stderr);
    assert_eq!(s.read("a"), "precious");
}

#[test]
fn cp_onto_a_symlink_to_the_source_keeps_its_content() {
    let s = Scratch::new("cp-same-link");
    s.write("a", b"precious");
    symlink(s.path("a"), s.path("b")).unwrap();
    let r = run(&s.0, &format!("cp {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("same file"), "{}", r.stderr);
    assert_eq!(s.read("a"), "precious");
}

#[test]
fn cp_continues_past_a_missing_source_and_exits_1() {
    let s = Scratch::new("cp-accumulate");
    s.write("a", b"A");
    s.mkdir("dst");
    let r = run(
        &s.0,
        &format!("cp {} {} {}", s.q("missing"), s.q("a"), s.q("dst")),
    );
    assert_eq!(r.code, 1);
    assert_eq!(s.read("dst/a"), "A");
    assert!(r.stderr.contains("missing"), "{}", r.stderr);
}

#[test]
fn cp_with_one_operand_is_a_usage_error() {
    let s = Scratch::new("cp-one");
    s.write("a", b"A");
    let r = run(&s.0, &format!("cp {}", s.q("a")));
    assert_eq!(r.code, 2, "{}", r.stderr);
    let r = run(&s.0, "cp");
    assert_eq!(r.code, 2, "{}", r.stderr);
}

#[test]
fn undo_restores_every_target_a_multi_source_cp_replaced() {
    let s = Scratch::new("cp-undo");
    s.write("a", b"A");
    s.write("b", b"B");
    s.write("dst/a", b"old-a");
    s.write("dst/b", b"old-b");
    let r = run(
        &s.0,
        &format!("cp {} {} {}; undo", s.q("a"), s.q("b"), s.q("dst")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("dst/a"), "old-a");
    assert_eq!(s.read("dst/b"), "old-b");
}

#[test]
fn mv_moves_every_source_into_the_target_directory() {
    let s = Scratch::new("mv-many");
    s.write("a", b"A");
    s.write("b", b"B");
    s.mkdir("dst");
    let r = run(
        &s.0,
        &format!("mv {} {} {}", s.q("a"), s.q("b"), s.q("dst")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("dst/a"), "A");
    assert_eq!(s.read("dst/b"), "B");
    assert!(!s.exists("a") && !s.exists("b"));
}

#[test]
fn mv_with_many_sources_needs_a_directory_target() {
    let s = Scratch::new("mv-many-file");
    s.write("a", b"A");
    s.write("b", b"B");
    s.write("c", b"C");
    let r = run(&s.0, &format!("mv {} {} {}", s.q("a"), s.q("b"), s.q("c")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("not a directory"), "{}", r.stderr);
    assert_eq!(s.read("a"), "A");
    assert_eq!(s.read("b"), "B");
    assert_eq!(s.read("c"), "C");
}

#[test]
fn mv_no_clobber_keeps_both_files() {
    let s = Scratch::new("mv-noclobber");
    s.write("a", b"new");
    s.write("b", b"old");
    let r = run(&s.0, &format!("mv -n {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("a"), "new");
    assert_eq!(s.read("b"), "old");
}

#[test]
fn mv_replaces_an_existing_target_by_default_and_force_does_too() {
    let s = Scratch::new("mv-replace");
    s.write("a", b"new");
    s.write("b", b"old");
    let r = run(&s.0, &format!("mv {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "new");
    assert!(!s.exists("a"));
    s.write("a2", b"newer");
    let r = run(&s.0, &format!("mv -f {} {}", s.q("a2"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "newer");
    assert!(!s.exists("a2"));
}

#[test]
fn mv_dash_n_and_dash_f_override_each_other_by_position() {
    let s = Scratch::new("mv-override");
    s.write("a", b"new");
    s.write("b", b"old");
    let r = run(&s.0, &format!("mv -f -n {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "old");
    assert_eq!(s.read("a"), "new");
    let r = run(&s.0, &format!("mv -n -f {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "new");
    assert!(!s.exists("a"));
}

#[test]
fn cp_dash_n_and_dash_f_override_each_other_by_position() {
    let s = Scratch::new("cp-override");
    s.write("a", b"new");
    s.write("b", b"old");
    let r = run(&s.0, &format!("cp -f -n {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "old");
    let r = run(&s.0, &format!("cp -n -f {} {}", s.q("a"), s.q("b")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("b"), "new");
}

#[test]
fn mv_refuses_to_move_a_directory_into_itself() {
    let s = Scratch::new("mv-nested");
    s.write("d/f", b"F");
    s.mkdir("d/sub");
    let r = run(&s.0, &format!("mv {} {}", s.q("d"), s.q("d/sub")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("subdirectory of itself"), "{}", r.stderr);
    assert_eq!(s.read("d/f"), "F");
}

#[test]
fn mv_file_onto_itself_keeps_its_content() {
    let s = Scratch::new("mv-same");
    s.write("a", b"precious");
    let r = run(&s.0, &format!("mv {} {}", s.q("a"), s.q("a")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("same file"), "{}", r.stderr);
    assert_eq!(s.read("a"), "precious");
}

#[test]
fn mv_continues_past_a_missing_source_and_exits_1() {
    let s = Scratch::new("mv-accumulate");
    s.write("a", b"A");
    s.mkdir("dst");
    let r = run(
        &s.0,
        &format!("mv {} {} {}", s.q("missing"), s.q("a"), s.q("dst")),
    );
    assert_eq!(r.code, 1);
    assert_eq!(s.read("dst/a"), "A");
    assert!(r.stderr.contains("missing"), "{}", r.stderr);
}

#[test]
fn mv_into_a_directory_replaces_an_existing_entry_of_the_same_name() {
    let s = Scratch::new("mv-into");
    s.write("a", b"new");
    s.write("dst/a", b"old");
    let r = run(&s.0, &format!("mv {} {}", s.q("a"), s.q("dst")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("dst/a"), "new");
}

#[test]
fn mv_across_filesystems_falls_back_to_copy_and_remove() {
    let s = Scratch::new("mv-xdev");
    let home_dev = fs::metadata(&s.0).unwrap().dev();
    let Some(other) = ["/dev/shm", "/var/tmp", "/run/user"]
        .iter()
        .map(PathBuf::from)
        .find(|p| fs::metadata(p).is_ok_and(|m| m.dev() != home_dev && m.is_dir()))
    else {
        eprintln!("skipped: no second filesystem available");
        return;
    };
    let dst = other.join(format!("koa-fs-xdev-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dst);
    if fs::create_dir_all(&dst).is_err() {
        eprintln!("skipped: {} not writable", other.display());
        return;
    }
    s.write("tree/sub/f", b"F");
    s.write("file", b"X");
    let r = run(
        &s.0,
        &format!("mv {} {} '{}'", s.q("tree"), s.q("file"), dst.display()),
    );
    let tree_ok = fs::read_to_string(dst.join("tree/sub/f")).ok();
    let file_ok = fs::read_to_string(dst.join("file")).ok();
    let left_over = s.exists("tree") || s.exists("file");
    let _ = fs::remove_dir_all(&dst);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(tree_ok.as_deref(), Some("F"));
    assert_eq!(file_ok.as_deref(), Some("X"));
    assert!(!left_over, "sources must be removed after the copy");
}

#[test]
fn undo_restores_every_source_and_target_of_a_multi_source_mv() {
    let s = Scratch::new("mv-undo");
    s.write("a", b"A");
    s.write("b", b"B");
    s.write("dst/a", b"old-a");
    let r = run(
        &s.0,
        &format!("mv {} {} {}; undo", s.q("a"), s.q("b"), s.q("dst")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(s.read("a"), "A");
    assert_eq!(s.read("b"), "B");
    assert_eq!(s.read("dst/a"), "old-a");
    assert!(!s.exists("dst/b"));
}

#[test]
fn mkdir_creates_the_other_operands_when_one_fails() {
    let s = Scratch::new("mkdir-accumulate");
    let r = run(
        &s.0,
        &format!("mkdir {} {} {}", s.q("no/parent"), s.q("c"), s.q("d")),
    );
    assert_eq!(r.code, 1);
    assert!(s.exists("c") && s.exists("d"));
    assert_eq!(r.stderr.matches("koa:").count(), 1, "{}", r.stderr);
}

#[test]
fn mkdir_verbose_names_each_created_directory() {
    let s = Scratch::new("mkdir-verbose");
    let r = run(&s.0, &format!("mkdir -pv {}", s.q("x/y")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", r.stdout);
    let x = s.path("x").display().to_string();
    let xy = s.path("x/y").display().to_string();
    assert!(
        lines[0].contains(&x) && !lines[0].contains(&xy),
        "{}",
        r.stdout
    );
    assert!(lines[1].contains(&xy), "{}", r.stdout);
    assert!(s.exists("x/y"));
}

#[test]
fn cat_dash_operand_reads_stdin_between_files() {
    let s = Scratch::new("cat-dash");
    s.write("a", b"A\n");
    s.write("b", b"B\n");
    let r = koa(&s.0, format!("cat --raw {} - {}", s.q("a"), s.q("b")))
        .stdin(b"mid\n")
        .run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "A\nmid\nB\n");
}

#[test]
fn cat_dash_operand_reads_stdin_once() {
    let s = Scratch::new("cat-dash-once");
    let r = koa(&s.0, "cat --raw - -").stdin(b"once\n").run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "once\n");
}

#[test]
fn cat_dash_operand_is_stdin_in_hashline_mode() {
    let s = Scratch::new("cat-dash-hash");
    s.write("a", b"A\n");
    let r = koa(&s.0, format!("cat {} -", s.q("a")))
        .stdin(b"from-stdin\n")
        .run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("from-stdin"), "{}", r.stdout);
    assert!(r.stdout.contains("<stdin>"), "{}", r.stdout);
}

#[test]
fn cat_raw_passes_binary_bytes_through_exactly() {
    let s = Scratch::new("cat-binary");
    let bytes: &[u8] = &[0xff, 0xfe, b'\r', b'\n', 0x00, 0x80];
    s.write("bin", bytes);
    let out = Command::new(KOA)
        .arg("-c")
        .arg(format!("cat --raw {}", s.q("bin")))
        .current_dir(&s.0)
        .stdin(Stdio::null())
        .output()
        .expect("spawn koa");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout, bytes);
}

#[test]
fn cat_hashline_mode_does_not_fail_on_invalid_utf8() {
    let s = Scratch::new("cat-invalid-utf8");
    s.write("bin", b"ok line\n\xff\xfe broken\n");
    let r = run(&s.0, &format!("cat {}", s.q("bin")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("ok line"), "{}", r.stdout);
    assert!(r.stdout.contains("broken"), "{}", r.stdout);
}

#[test]
fn cat_continues_after_a_missing_file_and_exits_1() {
    let s = Scratch::new("cat-accumulate");
    s.write("a", b"A\n");
    s.write("b", b"B\n");
    let r = run(
        &s.0,
        &format!("cat --raw {} {} {}", s.q("a"), s.q("missing"), s.q("b")),
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stdout, "A\nB\n");
    assert!(r.stderr.contains("missing"), "{}", r.stderr);
}

#[test]
fn cat_directory_operand_is_an_error() {
    let s = Scratch::new("cat-dir");
    s.mkdir("d");
    let r = run(&s.0, &format!("cat {}", s.q("d")));
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("Is a directory"), "{}", r.stderr);
}

#[test]
fn cat_raw_number_flag_numbers_the_lines() {
    let s = Scratch::new("cat-n");
    s.write("a", b"x\ny\n");
    let r = run(&s.0, &format!("cat --raw -n {}", s.q("a")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "     1\tx\n     2\ty\n");
}

#[test]
fn head_raw_is_byte_exact() {
    let s = Scratch::new("head-bytes");
    s.write("f", b"one\r\ntwo\r\nthree");
    let out = Command::new(KOA)
        .arg("-c")
        .arg(format!("head --raw -n 3 {}", s.q("f")))
        .current_dir(&s.0)
        .stdin(Stdio::null())
        .output()
        .expect("spawn koa");
    assert_eq!(out.stdout, b"one\r\ntwo\r\nthree");
}

#[test]
fn tail_raw_is_byte_exact() {
    let s = Scratch::new("tail-bytes");
    s.write("f", b"one\r\ntwo\r\nthree");
    let out = Command::new(KOA)
        .arg("-c")
        .arg(format!("tail --raw -n 2 {}", s.q("f")))
        .current_dir(&s.0)
        .stdin(Stdio::null())
        .output()
        .expect("spawn koa");
    assert_eq!(out.stdout, b"two\r\nthree");
}

#[test]
fn head_and_tail_byte_counts_are_raw() {
    let s = Scratch::new("headtail-bytes");
    s.write("f", b"abcdef");
    assert_eq!(run(&s.0, &format!("head -c 3 {}", s.q("f"))).stdout, "abc");
    assert_eq!(run(&s.0, &format!("tail -c 2 {}", s.q("f"))).stdout, "ef");
}

#[test]
fn tail_plus_count_starts_at_that_line() {
    let s = Scratch::new("tail-plus");
    s.write("f", b"h\n1\n2\n3\n");
    assert_eq!(
        run(&s.0, &format!("tail --raw -n +2 {}", s.q("f"))).stdout,
        "1\n2\n3\n"
    );
}

#[test]
fn tail_raw_returns_the_last_lines_of_a_large_file_with_invalid_utf8() {
    let s = Scratch::new("tail-big");
    let mut content = vec![0xff];
    content.extend_from_slice(b"irst\n");
    content.extend_from_slice(&vec![b'x'; 200_000]);
    content.extend_from_slice(b"\nlast\n");
    s.write("big", &content);
    let r = run(&s.0, &format!("tail --raw -n 2 {}", s.q("big")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let expected = format!("{}\nlast\n", "x".repeat(200_000));
    assert_eq!(r.stdout.len(), 200_006);
    assert_eq!(r.stdout, expected);
}

#[test]
fn tail_hashline_rows_carry_the_line_numbers_of_the_file() {
    let s = Scratch::new("tail-lines");
    s.write("f", b"a\nb\nc\nd\n");
    let r = run(&s.0, &format!("tail -n 2 {} | select line", s.q("f")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let numbers: Vec<u64> = r
        .stdout
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    assert_eq!(numbers, [3, 4], "{}", r.stdout);
}

#[test]
fn tail_raw_counts_lines_by_their_terminators() {
    let s = Scratch::new("tail-edges");
    let cases: [(&[u8], &str, &str); 8] = [
        (b"a\nb\nc\n", "-n 2", "b\nc\n"),
        (b"a\nb\nc", "-n 2", "b\nc"),
        (b"a\nb\nc\n", "-n 10", "a\nb\nc\n"),
        (b"a\n\n\n", "-n 2", "\n\n"),
        (b"", "-n 3", ""),
        (b"a\nb\n", "-n 0", ""),
        (b"a\n", "-n +9", ""),
        (b"h\n1\n2\n", "-n +1", "h\n1\n2\n"),
    ];
    for (content, flags, expected) in cases {
        s.write("f", content);
        let r = run(&s.0, &format!("tail --raw {flags} {}", s.q("f")));
        assert_eq!(r.code, 0, "{flags}: {}", r.stderr);
        assert_eq!(r.stdout, expected, "tail {flags} of {content:?}");
    }
    s.write("f", b"a\nb\nc\n");
    let r = run(&s.0, &format!("tail -c +3 {}", s.q("f")));
    assert_eq!(r.stdout, "b\nc\n");
}

#[test]
fn head_and_tail_let_the_last_of_dash_n_and_dash_c_decide() {
    let s = Scratch::new("headtail-unit");
    s.write("f", b"abc\ndef\n");
    assert_eq!(
        run(&s.0, &format!("head --raw -c 2 -n 1 {}", s.q("f"))).stdout,
        "abc\n"
    );
    assert_eq!(
        run(&s.0, &format!("head --raw -n 1 -c 2 {}", s.q("f"))).stdout,
        "ab"
    );
    assert_eq!(
        run(&s.0, &format!("tail --raw -c 2 -n 1 {}", s.q("f"))).stdout,
        "def\n"
    );
    assert_eq!(
        run(&s.0, &format!("tail --raw -n 1 -c 2 {}", s.q("f"))).stdout,
        "f\n"
    );
}

#[test]
fn head_and_tail_continue_after_a_missing_file() {
    let s = Scratch::new("headtail-accumulate");
    s.write("a", b"A\n");
    s.write("b", b"B\n");
    for cmd in ["head", "tail"] {
        let r = run(
            &s.0,
            &format!("{cmd} --raw {} {} {}", s.q("a"), s.q("missing"), s.q("b")),
        );
        assert_eq!(r.code, 1, "{cmd}");
        assert!(
            r.stdout.contains('A') && r.stdout.contains('B'),
            "{cmd}: {}",
            r.stdout
        );
        assert!(r.stderr.contains("missing"), "{cmd}: {}", r.stderr);
    }
}

#[test]
fn head_and_tail_raw_label_several_files() {
    let s = Scratch::new("headtail-headers");
    s.write("a", b"A\n");
    s.write("b", b"B\n");
    let expected = format!(
        "==> {} <==\nA\n\n==> {} <==\nB\n",
        s.path("a").display(),
        s.path("b").display()
    );
    for cmd in ["head", "tail"] {
        let r = run(&s.0, &format!("{cmd} --raw {} {}", s.q("a"), s.q("b")));
        assert_eq!(r.stdout, expected, "{cmd}");
        let r = run(&s.0, &format!("{cmd} --raw -q {} {}", s.q("a"), s.q("b")));
        assert_eq!(r.stdout, "A\nB\n", "{cmd}");
    }
}

#[test]
fn head_keeps_working_on_a_pipe_and_on_stdin() {
    let s = Scratch::new("head-pipe");
    let r = koa(&s.0, "head --raw -n 1").stdin(b"x\r\ny\r\n").run();
    assert_eq!(r.stdout, "x\r\n");
    let r = run(&s.0, "echo -n 'a\nb\nc' | tail -n 2");
    assert_eq!(r.stdout, "b\nc");
}

fn names(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .skip(2)
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect()
}

#[test]
fn ls_hides_dotfiles_unless_all_is_given() {
    let s = Scratch::new("ls-all");
    s.write("shown", b"");
    s.write(".hidden", b"");
    let plain = run(&s.0, &format!("ls {} | select name", s.q(".")));
    assert_eq!(names(&plain.stdout), ["shown"], "{}", plain.stdout);
    for flag in ["-a", "--all", "-A", "--almost-all"] {
        let r = run(&s.0, &format!("ls {flag} {} | select name", s.q(".")));
        assert_eq!(
            names(&r.stdout),
            [".hidden", "shown"],
            "{flag}: {}",
            r.stdout
        );
    }
}

#[test]
fn ls_long_option_with_an_a_in_it_does_not_show_hidden_files() {
    let s = Scratch::new("ls-substring");
    s.write("shown", b"");
    s.write(".hidden", b"");
    let r = run_builtins_only(&s, &format!("ls --author {}", s.q(".")));
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("--author"), "{}", r.stderr);
    assert!(!r.stdout.contains(".hidden"), "{}", r.stdout);
}

#[test]
fn ls_lists_the_directory_a_symlink_operand_points_to() {
    let s = Scratch::new("ls-symlink");
    s.write("real/inside", b"");
    symlink(s.path("real"), s.path("link")).unwrap();
    let r = run(&s.0, &format!("ls {} | select name", s.q("link")));
    assert_eq!(names(&r.stdout), ["inside"], "{}", r.stdout);
}

#[test]
fn ls_continues_past_a_missing_operand_and_exits_1() {
    let s = Scratch::new("ls-accumulate");
    s.write("d/inside", b"");
    let r = run(&s.0, &format!("ls {} {}", s.q("missing"), s.q("d")));
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stdout.contains("inside"), "{}", r.stdout);
    assert!(r.stderr.contains("missing"), "{}", r.stderr);
}

#[test]
fn ls_reports_an_unreadable_directory_instead_of_listing_nothing() {
    if nix_is_root() {
        return;
    }
    let s = Scratch::new("ls-unreadable");
    let d = s.mkdir("locked");
    fs::set_permissions(&d, fs::Permissions::from_mode(0o000)).unwrap();
    let r = run(&s.0, &format!("ls {}", s.q("locked")));
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("Permission denied"), "{}", r.stderr);
}

#[test]
fn ls_lists_a_broken_symlink_entry_as_a_symlink() {
    let s = Scratch::new("ls-broken-link");
    s.mkdir("d");
    symlink(s.path("nowhere"), s.path("d/dangling")).unwrap();
    let r = run(&s.0, &format!("ls {}", s.q("d")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains("dangling") && r.stdout.contains("symlink"),
        "{}",
        r.stdout
    );
}

#[test]
fn stat_reports_each_missing_operand_and_keeps_going() {
    let s = Scratch::new("stat-accumulate");
    s.write("a", b"x");
    let r = run(&s.0, &format!("stat {} {}", s.q("missing"), s.q("a")));
    assert_eq!(r.code, 1);
    assert!(
        r.stdout.contains(&s.path("a").display().to_string()),
        "{}",
        r.stdout
    );
    assert!(r.stderr.contains("missing"), "{}", r.stderr);
}

#[test]
fn cd_into_a_symlink_keeps_the_logical_directory() {
    let s = Scratch::new("cd-logical");
    s.mkdir("real");
    symlink(s.path("real"), s.path("link")).unwrap();
    let r = run(&s.0, &format!("cd {}; pwd", s.q("link")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains(&s.path("link").display().to_string()),
        "{}",
        r.stdout
    );
    let r = run(&s.0, &format!("cd -L {}; pwd", s.q("link")));
    assert!(
        r.stdout.contains(&s.path("link").display().to_string()),
        "{}",
        r.stdout
    );
}

#[test]
fn cd_dash_p_resolves_symlinks() {
    let s = Scratch::new("cd-physical");
    s.mkdir("real");
    symlink(s.path("real"), s.path("link")).unwrap();
    let r = run(&s.0, &format!("cd -P {}; pwd", s.q("link")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains(&s.path("real").display().to_string()),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("link"), "{}", r.stdout);
}

#[test]
fn pwd_dash_p_resolves_and_dash_l_keeps_the_logical_path() {
    let s = Scratch::new("pwd-modes");
    s.mkdir("real");
    symlink(s.path("real"), s.path("link")).unwrap();
    let r = run(&s.0, &format!("cd {}; pwd -P", s.q("link")));
    assert!(
        r.stdout.contains(&s.path("real").display().to_string()),
        "{}",
        r.stdout
    );
    let r = run(&s.0, &format!("cd {}; pwd -L", s.q("link")));
    assert!(
        r.stdout.contains(&s.path("link").display().to_string()),
        "{}",
        r.stdout
    );
}

#[test]
fn cd_dotdot_after_entering_a_symlink_returns_to_the_logical_parent() {
    let s = Scratch::new("cd-dotdot");
    s.mkdir("outer/real");
    symlink(s.path("outer/real"), s.path("link")).unwrap();
    let r = run(&s.0, &format!("cd {}; cd ..; pwd", s.q("link")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let scratch = s.0.display().to_string();
    assert!(
        r.stdout.lines().any(|l| l.trim() == scratch),
        "{}",
        r.stdout
    );
}

#[test]
fn cd_dash_returns_to_the_previous_directory_and_prints_it() {
    let s = Scratch::new("cd-dash");
    s.mkdir("a");
    s.mkdir("b");
    let r = run(
        &s.0,
        &format!("cd {}; cd {}; cd -; pwd", s.q("a"), s.q("b")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let a = s.path("a").display().to_string();
    let hits = r.stdout.lines().filter(|l| l.trim() == a).count();
    assert_eq!(hits, 2, "cd - prints {a} and pwd confirms it: {}", r.stdout);
}

#[test]
fn cd_keeps_oldpwd_and_pwd_in_the_environment() {
    let s = Scratch::new("cd-oldpwd");
    s.mkdir("a");
    let r = run(&s.0, &format!("cd {}; echo $PWD $OLDPWD", s.q("a")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let want = format!("{} {}", s.path("a").display(), s.0.display());
    assert_eq!(r.stdout.trim(), want);
}

#[test]
fn cd_without_home_is_an_error_and_stays_put() {
    let s = Scratch::new("cd-nohome");
    let r = koa(&s.0, "cd").without_env("HOME").run();
    assert_ne!(r.code, 0, "{}", r.stdout);
    assert!(r.stderr.contains("HOME"), "{}", r.stderr);
    let r = koa(&s.0, "cd; pwd").without_env("HOME").run();
    let here = s.0.display().to_string();
    assert!(
        r.stdout.lines().any(|l| l.trim() == here),
        "a failed cd must not move the shell: {}",
        r.stdout
    );
}

#[test]
fn cd_without_arguments_goes_home() {
    let s = Scratch::new("cd-home");
    let home = s.mkdir("home");
    let r = koa(&s.0, "cd; pwd")
        .env("HOME", home.display().to_string())
        .run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains(&home.display().to_string()),
        "{}",
        r.stdout
    );
}

#[test]
fn cd_searches_cdpath_and_prints_the_directory_it_chose() {
    let s = Scratch::new("cd-cdpath");
    s.mkdir("base/target");
    s.mkdir("elsewhere");
    let cdpath = format!(
        "{}:{}",
        s.path("elsewhere").display(),
        s.path("base").display()
    );
    let r = koa(&s.0, "cd target; pwd").env("CDPATH", &cdpath).run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    let want = s.path("base/target").display().to_string();
    assert_eq!(
        r.stdout.lines().filter(|l| l.trim() == want).count(),
        2,
        "cd prints the CDPATH hit once and pwd confirms it: {}",
        r.stdout
    );
}

#[test]
fn cd_prefers_the_current_directory_when_cdpath_lists_it_first() {
    let s = Scratch::new("cd-cdpath-dot");
    s.mkdir("here/target");
    s.mkdir("base/target");
    let cdpath = format!(".:{}", s.path("base").display());
    let cwd = s.path("here");
    let r = koa(&cwd, "cd target; pwd").env("CDPATH", &cdpath).run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout
            .contains(&s.path("here/target").display().to_string()),
        "{}",
        r.stdout
    );
}

#[test]
fn cd_dot_relative_paths_ignore_cdpath() {
    let s = Scratch::new("cd-cdpath-rel");
    s.mkdir("here/target");
    s.mkdir("base/target");
    let cwd = s.path("here");
    let r = koa(&cwd, "cd ./target; pwd")
        .env("CDPATH", s.path("base").display().to_string())
        .run();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout
            .contains(&s.path("here/target").display().to_string()),
        "{}",
        r.stdout
    );
}

#[test]
fn cd_to_a_missing_directory_or_a_file_fails_and_stays_put() {
    let s = Scratch::new("cd-fail");
    s.write("file", b"x");
    let here = s.0.display().to_string();
    for operand in ["missing", "file"] {
        let target = s.q(operand);
        let r = run(&s.0, &format!("cd {target}"));
        assert_eq!(r.code, 1, "{operand}: {}", r.stderr);
        assert!(r.stderr.contains(operand), "{operand}: {}", r.stderr);
        let r = run(&s.0, &format!("cd {target}; pwd"));
        assert!(
            r.stdout.lines().any(|l| l.trim() == here),
            "cd {operand} must leave the directory unchanged: {}",
            r.stdout
        );
    }
    let r = run(&s.0, &format!("cd {}", s.q("file")));
    assert!(
        r.stderr.contains("Not a directory") || r.stderr.contains("not a directory"),
        "{}",
        r.stderr
    );
}

#[test]
fn cd_dash_p_and_dash_l_override_each_other_by_position() {
    let s = Scratch::new("cd-override");
    s.mkdir("real");
    symlink(s.path("real"), s.path("link")).unwrap();
    let link = s.path("link").display().to_string();
    let real = s.path("real").display().to_string();
    let r = run(&s.0, &format!("cd -P -L {}; pwd", s.q("link")));
    assert!(r.stdout.lines().any(|l| l.trim() == link), "{}", r.stdout);
    let r = run(&s.0, &format!("cd -L -P {}; pwd", s.q("link")));
    assert!(r.stdout.lines().any(|l| l.trim() == real), "{}", r.stdout);
}

#[test]
fn cd_dotdot_past_the_root_stays_at_the_root() {
    let s = Scratch::new("cd-root");
    let r = run(&s.0, "cd /../..; pwd");
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.lines().any(|l| l.trim() == "/"), "{}", r.stdout);
}

#[test]
fn cd_with_two_operands_or_an_unknown_option_is_a_usage_error() {
    let s = Scratch::new("cd-usage");
    s.mkdir("a");
    s.mkdir("b");
    assert_eq!(run(&s.0, &format!("cd {} {}", s.q("a"), s.q("b"))).code, 2);
    assert_eq!(run(&s.0, &format!("cd -Z {}", s.q("a"))).code, 2);
}

#[test]
fn export_name_without_a_value_keeps_the_existing_variable() {
    let s = Scratch::new("export-name");
    let r = run(&s.0, "FOO=bar; export FOO; echo $FOO");
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "bar");
}

#[test]
fn export_handles_assignments_and_bare_names_together() {
    let s = Scratch::new("export-mixed");
    let r = run(&s.0, "B=two; export A=1 B; echo $A $B");
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "1 two");
}

#[test]
fn exported_variable_reaches_external_commands() {
    let s = Scratch::new("export-external");
    let r = run(
        &s.0,
        "export KOA_FS_EXPORT=seen; /bin/sh -c 'echo $KOA_FS_EXPORT'",
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "seen");
}

#[test]
fn export_accepts_an_unset_name_and_rejects_an_invalid_one() {
    let s = Scratch::new("export-bad");
    let r = run(&s.0, "export NOT_SET_ANYWHERE_KOA");
    assert_eq!(r.code, 0, "{}", r.stderr);
    let r = run(&s.0, "export 1bad=x");
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("1bad"), "{}", r.stderr);
}

#[test]
fn grep_does_not_recurse_into_a_directory_without_dash_r() {
    let s = Scratch::new("grep-dir");
    s.write("d/inner.txt", b"needle\n");
    let r = run(&s.0, &format!("grep needle {}", s.q("d")));
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("Is a directory"), "{}", r.stderr);
    assert!(!r.stdout.contains("needle"), "{}", r.stdout);
}

#[test]
fn grep_recurses_into_a_directory_with_dash_r() {
    let s = Scratch::new("grep-r");
    s.write("d/inner.txt", b"needle\n");
    for flag in ["-r", "-R", "--recursive"] {
        let r = run(&s.0, &format!("grep {flag} needle {}", s.q("d")));
        assert_eq!(r.code, 0, "{flag}: {}", r.stderr);
        assert!(r.stdout.contains("needle"), "{flag}: {}", r.stdout);
    }
}

#[test]
fn grep_reads_a_named_file_and_options_after_the_pattern() {
    let s = Scratch::new("grep-file");
    s.write("f", b"Needle\nhay\n");
    let r = run(&s.0, &format!("grep needle -i {}", s.q("f")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("Needle"), "{}", r.stdout);
}

#[test]
fn find_keeps_its_single_dash_predicates() {
    let s = Scratch::new("find-predicates");
    s.write("src/main.rs", b"");
    s.write("src/lib.txt", b"");
    let r = run(
        &s.0,
        &format!("find {} -name '*.rs' -type f | select path", s.q(".")),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains("main.rs") && !r.stdout.contains("lib.txt"),
        "{}",
        r.stdout
    );
    let r = run(&s.0, &format!("find {} -t d", s.q(".")));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let r = run(&s.0, &format!("find {} -maxdepth x", s.q(".")));
    assert_eq!(r.code, 2, "{}", r.stderr);
}

#[test]
fn kill_rejects_an_unknown_option_without_signalling() {
    let s = Scratch::new("kill-unknown");
    let mut victim = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn sleep");
    let r = run_builtins_only(&s, &format!("kill --bogus {}", victim.id()));
    let still_running = victim.try_wait().expect("try_wait").is_none();
    victim.kill().expect("kill sleep");
    victim.wait().expect("reap sleep");
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("--bogus"), "{}", r.stderr);
    assert!(
        still_running,
        "kill signalled the process despite the usage error"
    );
}

#[test]
fn schemas_and_help_list_flags_as_their_spellings() {
    let s = Scratch::new("help-flags");
    let r = run(&s.0, "help rm");
    assert_eq!(r.code, 0, "{}", r.stderr);
    for spelling in ["-r", "--recursive", "-f", "--force"] {
        assert!(r.stdout.contains(spelling), "{spelling}: {}", r.stdout);
    }
    let r = run(&s.0, "help find");
    assert!(
        r.stdout.contains("-name") && !r.stdout.contains("--name"),
        "{}",
        r.stdout
    );
}
