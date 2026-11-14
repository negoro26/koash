use std::borrow::Cow;
use std::sync::Arc;

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
    strict_options: false,
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
        let batch = input.into_batch();
        let mut keep_cols = Vec::new();
        let mut keep_indices = Vec::new();
        for col in args {
            if let Some(idx) = batch.col_index(col) {
                keep_cols.push(Cow::Owned(col.clone()));
                keep_indices.push(idx);
            }
        }
        if keep_cols.is_empty() {
            return Ok(Value::from_batch(RecordBatch::empty()));
        }
        let nrows = batch.nrows();
        let mut out_cells = Vec::with_capacity(nrows * keep_cols.len());
        for r in 0..nrows {
            let row = batch.row(r);
            for &idx in &keep_indices {
                out_cells.push(row[idx].clone());
            }
        }
        Ok(Value::from_batch(RecordBatch {
            columns: Arc::from(keep_cols),
            cells: out_cells,
        }))
    }
}
