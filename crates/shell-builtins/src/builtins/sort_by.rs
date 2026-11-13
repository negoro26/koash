use shell_kernel::Session;
use shell_value::{RecordBatch, StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct SortBy;

static SIGNATURE: Signature = Signature {
    name: "sort-by",
    aliases: &["sort_by"],
    description: "Sort structured records by column",
    flags: &["--desc"],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for SortBy {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut desc = false;
        let mut col: Option<String> = None;
        for arg in args {
            if arg == "--desc" || arg == "-r" {
                desc = true;
            } else {
                col = Some(arg.clone());
            }
        }
        let Some(col) = col else {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "sort-by requires a column name",
                None,
                None,
            ));
        };
        let batch = input.into_batch();
        let Some(col_idx) = batch.col_index(&col) else {
            return Ok(Value::from_batch(batch));
        };
        let nrows = batch.nrows();
        let ncols = batch.ncols();
        let mut order: Vec<usize> = (0..nrows).collect();
        order.sort_by(|&a, &b| {
            let va = &batch.row(a)[col_idx];
            let vb = &batch.row(b)[col_idx];
            let ord = match (va.as_f64(), vb.as_f64()) {
                (Some(na), Some(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
                _ => {
                    let sa = va.as_str().unwrap_or("");
                    let sb = vb.as_str().unwrap_or("");
                    sa.cmp(sb)
                }
            };
            if desc { ord.reverse() } else { ord }
        });
        let mut out_cells = Vec::with_capacity(nrows * ncols);
        for idx in order {
            out_cells.extend_from_slice(batch.row(idx));
        }
        Ok(Value::from_batch(RecordBatch {
            columns: batch.columns,
            cells: out_cells,
        }))
    }
}
