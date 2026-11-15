use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Rm;

static SIGNATURE: Signature = Signature {
    name: "rm",
    aliases: &[],
    description: "Remove files or directories",
    flags: &["-r", "-f"],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Rm {
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
        let mut force = false;
        let mut targets = Vec::new();
        for arg in args {
            if arg.starts_with('-') {
                if arg.contains('r') || arg.contains('R') {
                    recursive = true;
                }
                if arg.contains('f') {
                    force = true;
                }
            } else {
                targets.push(arg.clone());
            }
        }
        for t in targets {
            let p = resolve_path(session, &t);
            if !p.exists() {
                if force {
                    continue;
                }
                return Err(StructuredDiagnostic::new(
                    "E_RM",
                    format!("{}: no such file or directory", p.display()),
                    None,
                    None,
                ));
            }
            if p.is_dir() && !recursive {
                session.record_preimage(&p);
            } else {
                session.record_tree(&p);
            }
            let res = if p.is_dir() {
                if recursive {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_dir(&p)
                }
            } else {
                std::fs::remove_file(&p)
            };
            res.map_err(|e| StructuredDiagnostic::from_io("E_RM", &e, None))?;
            session.record_mutation(p.to_string_lossy().to_string());
        }
        Ok(Value::Empty)
    }
}
