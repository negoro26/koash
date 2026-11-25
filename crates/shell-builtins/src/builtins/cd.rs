use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use nix::unistd::{AccessFlags, access};
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
    strict_options: true,
};

const ENOENT: i32 = 2;
const ENOTDIR: i32 = 20;

struct Candidate {
    path: PathBuf,
    from_cdpath: bool,
}

impl Builtin for Cd {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

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

        let resolved = resolve(session, &dir, search_cdpath, physical)
            .map_err(|e| cd_error(format!("cd: {dir}: {}", describe(&e))))?;
        let text = resolved.path.to_string_lossy().to_string();
        enter(session, resolved.path);
        if announce || resolved.from_cdpath {
            return Ok(Value::from_bytes(format!("{text}\n").into_bytes()));
        }
        Ok(Value::Empty)
    }
}

pub(super) struct Resolved {
    pub(super) path: PathBuf,
    pub(super) from_cdpath: bool,
}

pub(super) fn resolve(
    session: &Session,
    dir: &str,
    search_cdpath: bool,
    physical: bool,
) -> io::Result<Resolved> {
    let mut best: Option<io::Error> = None;
    let mut chosen = None;
    for candidate in candidates(session, dir, search_cdpath) {
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
        return Err(best.unwrap_or_else(|| io::Error::from_raw_os_error(ENOENT)));
    };
    let path = if physical {
        candidate.path.canonicalize()?
    } else {
        normalized
    };
    Ok(Resolved {
        path,
        from_cdpath: candidate.from_cdpath,
    })
}

pub(super) fn enter(session: &mut Session, new_dir: PathBuf) {
    let old = session.cwd.to_string_lossy().to_string();
    session.env.insert("OLDPWD".to_string(), old);
    session
        .env
        .insert("PWD".to_string(), new_dir.to_string_lossy().to_string());
    session.cwd = new_dir;
}

fn cd_error(message: impl Into<String>) -> StructuredDiagnostic {
    StructuredDiagnostic::new("E_CD", message, None, None)
}

fn probe(path: &Path) -> io::Result<()> {
    if !fs::metadata(path)?.is_dir() {
        return Err(io::Error::from_raw_os_error(ENOTDIR));
    }
    access(path, AccessFlags::X_OK).map_err(io::Error::from)
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
