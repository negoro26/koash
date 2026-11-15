use shell_kernel::Session;
use shell_value::{CellVal, RecordBatch, StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Where;

static SIGNATURE: Signature = Signature {
    name: "where",
    aliases: &[],
    description: "Filter structured records by predicate",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Where {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        if args.len() < 3 {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "where requires `<field> <op> <value>`",
                None,
                Some("example: ls | where size > 10kb".to_string()),
            ));
        }
        let field = &args[0];
        let op = &args[1];
        let rhs_raw = &args[2];
        let batch = input.into_batch();
        let Some(col_idx) = batch.col_index(field) else {
            return Ok(Value::from_batch(RecordBatch::new(batch.columns)));
        };

        let mut out_cells = Vec::new();
        for r in 0..batch.nrows() {
            let row = batch.row(r);
            if eval_cell_predicate(&row[col_idx], op, rhs_raw) {
                out_cells.extend_from_slice(row);
            }
        }
        Ok(Value::from_batch(RecordBatch {
            columns: batch.columns,
            cells: out_cells,
        }))
    }
}

fn eval_cell_predicate(lhs: &CellVal, op: &str, rhs_raw: &str) -> bool {
    if let Some(lhs_num) = lhs.as_f64()
        && let Some(rhs_num) = parse_size_or_number(rhs_raw)
    {
        return match op {
            ">" => lhs_num > rhs_num,
            ">=" => lhs_num >= rhs_num,
            "<" => lhs_num < rhs_num,
            "<=" => lhs_num <= rhs_num,
            "==" | "=" => (lhs_num - rhs_num).abs() < f64::EPSILON,
            "!=" => (lhs_num - rhs_num).abs() >= f64::EPSILON,
            _ => false,
        };
    }

    let owned;
    let lhs_str = if let Some(s) = lhs.as_str() {
        s
    } else {
        owned = lhs.format_to_string();
        &owned
    };
    match op {
        "==" | "=" => lhs_str == rhs_raw,
        "!=" => lhs_str != rhs_raw,
        "contains" | "=~" => lhs_str.contains(rhs_raw),
        ">" => lhs_str > rhs_raw,
        "<" => lhs_str < rhs_raw,
        _ => false,
    }
}

fn parse_size_or_number(s: &str) -> Option<f64> {
    let lower = s.trim().to_ascii_lowercase();
    if let Ok(n) = lower.parse::<f64>() {
        return Some(n);
    }
    let suffixes = [
        ("gb", 1024.0 * 1024.0 * 1024.0),
        ("mb", 1024.0 * 1024.0),
        ("kb", 1024.0),
        ("b", 1.0),
    ];
    for (suf, mult) in suffixes {
        if let Some(num_part) = lower.strip_suffix(suf)
            && let Ok(n) = num_part.trim().parse::<f64>()
        {
            return Some(n * mult);
        }
    }
    None
}
