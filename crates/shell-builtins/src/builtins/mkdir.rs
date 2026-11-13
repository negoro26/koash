use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Mkdir;

static SIGNATURE: Signature = Signature {
    name: "mkdir",
    aliases: &[],
    description: "Create directories",
    flags: &["-p"],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Mkdir {
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
        let mut targets = Vec::new();
        let mut parse_flags = true;
        for arg in args {
            if parse_flags && arg == "--" {
                parse_flags = false;
            } else if parse_flags && arg.starts_with('-') && arg.len() > 1 {
                if arg.contains('p') {
                    recursive = true;
                }
            } else {
                targets.push(arg.clone());
            }
        }
        for t in targets {
            let p = resolve_path(session, &t);
            if let Some(top) = p.ancestors().take_while(|a| !a.exists()).last() {
                session.record_preimage(top);
            }
            let res = if recursive {
                std::fs::create_dir_all(&p)
            } else {
                std::fs::create_dir(&p)
            };
            res.map_err(|e| StructuredDiagnostic::from_io("E_MKDIR", &e, None))?;
            session.record_mutation(p.to_string_lossy().to_string());
        }
        Ok(Value::Empty)
    }
}
