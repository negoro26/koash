use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::usage_error;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Undo;

static SIGNATURE: Signature = Signature {
    name: "undo",
    aliases: &[],
    description: "Undo the most recent transactional micro-commit",
    flags: &[],
    columns: crate::builtin::columns!["tx_id", "restored_files"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: false,
};

impl Builtin for Undo {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        if let Some(extra) = Args::parse(&SIGNATURE, args)?.operands.first() {
            return Err(usage_error(
                format!("undo takes no arguments, got '{extra}'"),
                "undo",
            ));
        }
        match session.undo_last()? {
            Some((tx_id, count)) => {
                let mut rec = StructuredRecord::new();
                rec.insert("tx_id".to_string(), JsonVal::String(tx_id));
                rec.insert("restored_files".to_string(), json!(count));
                Value::from_records(vec![rec])
            }
            None => Value::from_records(Vec::new()),
        }
    }
}
