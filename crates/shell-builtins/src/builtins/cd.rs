use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::usage_error;
use super::fs_ops::describe;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Cd;

static SIGNATURE: Signature = Signature {
    name: "cd",
    aliases: &[],
    description: "Change working directory",
    flags: &[
        Flag::short_switch(
            'L',
            "keep symlinks in the new directory as entered (default)",
        ),
        Flag::short_switch('P', "resolve symlinks in the new directory"),
    ],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

const ENOTDIR: i32 = 20;

/// A directory to try, and whether a `CDPATH` entry other than the current directory produced it.
struct Candidate {
    path: PathBuf,
    from_cdpath: bool,
}

impl Builtin for Cd {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// The new directory is the logical path, `..` folded away lexically (fish cd.rs:85,
    /// `normalize_path`), unless `-P` resolves it. `CDPATH` entries become a candidate list that is
    /// tried in order (fish path.rs:334-373, `path_apply_cdpath`). `cd -` and a `CDPATH` hit print
    /// the directory they chose.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        if args.operands.len() > 1 {
            return Err(usage_error("cd: too many arguments", "cd [-L|-P] [DIR]"));
        }
        let physical = args.last_of(&["L", "P"]) == Some("P");
        let operand = args.operands.first().copied();

        let (dir, search_cdpath, announce) = match operand {
            None => match session.env.get("HOME").filter(|h| !h.is_empty()) {
                Some(home) => (home, false, false),
                None => return Err(cd_error("cd: HOME not set")),
            },
            Some("-") => match session.env.get("OLDPWD").filter(|d| !d.is_empty()) {
                Some(old) => (old, false, true),
                None => return Err(cd_error("cd: OLDPWD not set")),
            },
            Some(dir) => (dir.to_string(), true, false),
        };

        let candidates = candidates(session, &dir, search_cdpath);
        let mut best: Option<io::Error> = None;
        let mut chosen = None;
        for candidate in candidates {
            let normalized = normalize(&candidate.path);
            match probe(&candidate.path).and_then(|()| probe(&normalized)) {
                Ok(()) => {
                    chosen = Some((candidate, normalized));
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    best.get_or_insert(e);
                }
                Err(e) if e.raw_os_error() == Some(ENOTDIR) => best = Some(e),
                Err(e) => {
                    best = Some(e);
                    break;
                }
            }
        }
        let Some((candidate, normalized)) = chosen else {
            let reason =
                best.map_or_else(|| "No such file or directory".to_string(), |e| describe(&e));
            return Err(cd_error(format!("cd: {dir}: {reason}")));
        };

        let new_dir = if physical {
            candidate
                .path
                .canonicalize()
                .map_err(|e| cd_error(format!("cd: {dir}: {}", describe(&e))))?
        } else {
            normalized
        };
        let old = session.cwd.to_string_lossy().to_string();
        let text = new_dir.to_string_lossy().to_string();
        session.env.insert("OLDPWD".to_string(), old);
        session.env.insert("PWD".to_string(), text.clone());
        session.cwd = new_dir;
        if announce || candidate.from_cdpath {
            return Ok(Value::from_bytes(format!("{text}\n").into_bytes()));
        }
        Ok(Value::Empty)
    }
}

fn cd_error(message: impl Into<String>) -> StructuredDiagnostic {
    StructuredDiagnostic::new("E_CD", message, None, None)
}

fn probe(path: &Path) -> io::Result<()> {
    if fs::metadata(path)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(ENOTDIR))
    }
}

fn candidates(session: &Session, dir: &str, search_cdpath: bool) -> Vec<Candidate> {
    let cwd = &session.cwd;
    let relative_to_cwd =
        dir == "." || dir == ".." || dir.starts_with("./") || dir.starts_with("../");
    if Path::new(dir).is_absolute() {
        return vec![Candidate {
            path: PathBuf::from(dir),
            from_cdpath: false,
        }];
    }
    if relative_to_cwd || !search_cdpath {
        return vec![Candidate {
            path: cwd.join(dir),
            from_cdpath: false,
        }];
    }
    let cdpath = session.env.get("CDPATH").unwrap_or_default();
    cdpath
        .split(':')
        .chain(std::iter::once("."))
        .map(|entry| {
            let here = entry.is_empty() || entry == ".";
            let base = if here { cwd.clone() } else { cwd.join(entry) };
            Candidate {
                path: base.join(dir),
                from_cdpath: !here,
            }
        })
        .collect()
}

/// Folds `.` and `..` away without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_folds_dots_lexically_and_stays_at_the_root() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), Path::new("/a/c/d"));
        assert_eq!(normalize(Path::new("/a/link/..")), Path::new("/a"));
        assert_eq!(normalize(Path::new("/../..")), Path::new("/"));
        assert_eq!(normalize(Path::new("/")), Path::new("/"));
    }
}
