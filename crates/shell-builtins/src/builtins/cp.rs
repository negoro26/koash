use std::path::Path;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Cp;

static SIGNATURE: Signature = Signature {
    name: "cp",
    aliases: &[],
    description: "Copy files or directories",
    flags: &["-r"],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Cp {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut recursive = false;
        let mut positional = Vec::new();
        for arg in args {
            if arg == "-r" || arg == "-R" {
                recursive = true;
            } else {
                positional.push(arg.clone());
            }
        }
        if positional.len() < 2 {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "cp requires source and destination",
                None,
                None,
            ));
        }
        let src = resolve_path(session, &positional[0]);
        let dst = resolve_path(session, &positional[1]);
        if src.is_dir() {
            if !recursive {
                return Err(StructuredDiagnostic::new(
                    "E_CP",
                    format!("{} is a directory (use -r)", src.display()),
                    None,
                    None,
                ));
            }
            record_copy_targets(session, &src, &dst);
            copy_dir_recursive(&src, &dst)
                .map_err(|e| StructuredDiagnostic::from_io("E_CP", &e, None))?;
        } else {
            let final_dst = if dst.is_dir() {
                match src.file_name() {
                    Some(name) => dst.join(name),
                    None => dst.clone(),
                }
            } else {
                dst.clone()
            };
            session.record_tree(&final_dst);
            std::fs::copy(&src, &final_dst)
                .map_err(|e| StructuredDiagnostic::from_io("E_CP", &e, None))?;
        }
        session.record_mutation(dst.to_string_lossy().to_string());
        Ok(Value::Empty)
    }
}

fn record_copy_targets(session: &mut Session, src: &Path, dst: &Path) {
    let mut pending = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((from, to)) = pending.pop() {
        let existed = std::fs::symlink_metadata(&to).is_ok();
        if !session.record_preimage(&to) {
            return;
        }
        if !existed {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&from) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push((entry.path(), to.join(entry.file_name())));
            } else if !session.record_preimage(&to.join(entry.file_name())) {
                return;
            }
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
