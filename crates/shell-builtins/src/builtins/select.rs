use shell_kernel::Session;
use shell_value::{RecordBatch, StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Select;

static SIGNATURE: Signature = Signature {
    name: "select",
    aliases: &[],
    description: "Project specific columns from structured records",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Select {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        if args.is_empty() {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "select requires at least one column name",
                None,
                Some("example: ls | select name size".to_string()),
            ));
        }
        let mut batch = input.into_batch();
        let keep: Vec<usize> = args.iter().filter_map(|col| batch.col_index(col)).collect();
        if keep.is_empty() {
            return Ok(Value::from_batch(RecordBatch::empty()));
        }
        batch.select_columns(&keep);
        Ok(Value::from_batch(batch))
    }
}
