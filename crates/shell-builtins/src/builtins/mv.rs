use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Mv;

static SIGNATURE: Signature = Signature {
    name: "mv",
    aliases: &[],
    description: "Move or rename files",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Mv {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        if args.len() < 2 {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "mv requires source and destination",
                None,
                None,
            ));
        }
        let src = resolve_path(session, &args[0]);
        let dst = resolve_path(session, &args[1]);
        let final_dst = if dst.is_dir() {
            match src.file_name() {
                Some(name) => dst.join(name),
                None => dst.clone(),
            }
        } else {
            dst
        };
        session.record_tree(&src);
        session.record_tree(&final_dst);
        std::fs::rename(&src, &final_dst)
            .map_err(|e| StructuredDiagnostic::from_io("E_MV", &e, None))?;
        session.record_mutation(src.to_string_lossy().to_string());
        session.record_mutation(final_dst.to_string_lossy().to_string());
        Ok(Value::Empty)
    }
}
