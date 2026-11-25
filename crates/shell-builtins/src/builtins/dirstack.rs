use shell_kernel::Session;
use shell_value::{Record, Rows, StructuredDiagnostic, Value, record};

use super::cd::{enter, resolve};
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
                let Some(top) = session
                    .dir_stack
                    .last()
                    .map(|p| p.to_string_lossy().to_string())
                else {
                    return Err(StructuredDiagnostic::new(
                        "E_DIRSTACK",
                        "pushd: no other directory",
                        None,
                        None,
                    ));
                };
                let old = session.cwd.clone();
                change_dir(session, "pushd", &top, false)?;
                if let Some(slot) = session.dir_stack.last_mut() {
                    *slot = old;
                }
            }
            [""] => return Err(dir_error("pushd: null directory")),
            ["-"] => {
                let Some(previous) = session.env.get("OLDPWD").filter(|d| !d.is_empty()) else {
                    return Err(dir_error("pushd: OLDPWD not set"));
                };
                let old = session.cwd.clone();
                change_dir(session, "pushd", &previous, false)?;
                session.dir_stack.push(old);
            }
            [rotation] if is_rotation(rotation) => {
                return Err(usage_error(
                    format!("pushd: {rotation}: stack rotation is not supported"),
                    "pushd [DIR]",
                ));
            }
            [dir] => {
                let old = session.cwd.clone();
                change_dir(session, "pushd", dir, true)?;
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
        let Some(top) = session
            .dir_stack
            .last()
            .map(|p| p.to_string_lossy().to_string())
        else {
            return Err(StructuredDiagnostic::new(
                "E_DIRSTACK",
                "popd: directory stack empty",
                None,
                None,
            ));
        };
        change_dir(session, "popd", &top, false)?;
        session.dir_stack.pop();
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

fn is_rotation(operand: &str) -> bool {
    operand
        .strip_prefix('+')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn dir_error(message: impl Into<String>) -> StructuredDiagnostic {
    StructuredDiagnostic::new("E_CD", message, None, None)
}

fn change_dir(
    session: &mut Session,
    who: &str,
    dir: &str,
    search_cdpath: bool,
) -> Result<(), StructuredDiagnostic> {
    let resolved = resolve(session, dir, search_cdpath, false)
        .map_err(|e| dir_error(format!("{who}: {dir}: {}", describe(&e))))?;
    enter(session, resolved.path);
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
