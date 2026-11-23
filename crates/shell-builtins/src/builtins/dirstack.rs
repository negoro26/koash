use std::fs;
use std::path::{Component, Path, PathBuf};

use shell_kernel::Session;
use shell_value::{Record, Rows, StructuredDiagnostic, Value, record};

use super::common::usage_error;
use super::fs_ops::describe;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Pushd;
pub(super) struct Popd;
pub(super) struct Dirs;

record! {
    struct DirRow<'a> {
        index: u64,
        #[text] path: &'a str,
    }
}

static PUSHD_SIG: Signature = Signature {
    name: "pushd",
    aliases: &[],
    description: "Push current directory onto the directory stack and change to DIR",
    flags: &[],
    columns: <DirRow<'static> as Record>::COLUMNS,
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: true,
};

static POPD_SIG: Signature = Signature {
    name: "popd",
    aliases: &[],
    description: "Pop top directory from the directory stack and change to it",
    flags: &[],
    columns: <DirRow<'static> as Record>::COLUMNS,
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: true,
};

static DIRS_SIG: Signature = Signature {
    name: "dirs",
    aliases: &[],
    description: "List the directory stack as a structured table",
    flags: &[],
    columns: <DirRow<'static> as Record>::COLUMNS,
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: true,
};

impl Builtin for Pushd {
    fn signature(&self) -> &'static Signature {
        &PUSHD_SIG
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&PUSHD_SIG, args)?;
        match parsed.operands.as_slice() {
            [] => {
                let Some(top) = session.dir_stack.pop() else {
                    return Err(StructuredDiagnostic::new(
                        "E_DIRSTACK",
                        "pushd: no other directory",
                        None,
                        None,
                    ));
                };
                let old = session.cwd.clone();
                change_dir(session, &top)?;
                session.dir_stack.push(old);
            }
            [dir] => {
                let target = resolve_target(&session.cwd, dir)?;
                let old = session.cwd.clone();
                change_dir(session, &target)?;
                session.dir_stack.push(old);
            }
            _ => return Err(usage_error("pushd: too many arguments", "pushd [DIR]")),
        }
        render_stack(session)
    }
}

impl Builtin for Popd {
    fn signature(&self) -> &'static Signature {
        &POPD_SIG
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&POPD_SIG, args)?;
        if !parsed.operands.is_empty() {
            return Err(usage_error("popd takes no arguments", "popd"));
        }
        let Some(top) = session.dir_stack.pop() else {
            return Err(StructuredDiagnostic::new(
                "E_DIRSTACK",
                "popd: directory stack empty",
                None,
                None,
            ));
        };
        change_dir(session, &top)?;
        render_stack(session)
    }
}

impl Builtin for Dirs {
    fn signature(&self) -> &'static Signature {
        &DIRS_SIG
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&DIRS_SIG, args)?;
        if !parsed.operands.is_empty() {
            return Err(usage_error("dirs takes no arguments", "dirs"));
        }
        render_stack(session)
    }
}

fn resolve_target(cwd: &Path, raw: &str) -> Result<PathBuf, StructuredDiagnostic> {
    let joined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        cwd.join(raw)
    };
    let mut normalized = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    match fs::metadata(&normalized) {
        Ok(meta) if meta.is_dir() => Ok(normalized),
        Ok(_) => Err(StructuredDiagnostic::new(
            "E_CD",
            format!("pushd: {raw}: Not a directory"),
            None,
            None,
        )),
        Err(e) => Err(StructuredDiagnostic::new(
            "E_CD",
            format!("pushd: {raw}: {}", describe(&e)),
            None,
            None,
        )),
    }
}

fn change_dir(session: &mut Session, target: &Path) -> Result<(), StructuredDiagnostic> {
    if !fs::metadata(target).is_ok_and(|m| m.is_dir()) {
        return Err(StructuredDiagnostic::new(
            "E_CD",
            format!("directory no longer exists: {}", target.display()),
            None,
            None,
        ));
    }
    let old = session.cwd.to_string_lossy().to_string();
    let next = target.to_string_lossy().to_string();
    session.env.insert("OLDPWD".to_string(), old);
    session.env.insert("PWD".to_string(), next);
    session.cwd = target.to_path_buf();
    Ok(())
}

fn render_stack(session: &Session) -> Result<Value, StructuredDiagnostic> {
    let mut rows = Rows::<DirRow<'static>>::with_capacity(session.dir_stack.len() + 1);
    let cwd = session.cwd.to_string_lossy();
    rows.push(&DirRow {
        index: 0,
        path: &cwd,
    });
    for (i, p) in session.dir_stack.iter().rev().enumerate() {
        let text = p.to_string_lossy();
        rows.push(&DirRow {
            index: (i + 1) as u64,
            path: &text,
        });
    }
    Ok(Value::from_batch(rows.finish()?))
}
