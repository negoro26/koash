use std::cmp::{Ordering, Reverse};

use shell_kernel::Session;
use shell_value::{Datum, StructuredDiagnostic, Value};

use super::common::usage_error;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct SortBy;

static SIGNATURE: Signature = Signature {
    name: "sort-by",
    aliases: &["sort_by"],
    description: "Sort structured records by column",
    flags: &[Flag::switch('r', "desc", "sort in descending order")],
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
        let args = Args::parse(&SIGNATURE, args)?;
        let desc = args.has("desc");
        let col = match args.operands.as_slice() {
            [col] => (*col).to_string(),
            operands => {
                let problem = if operands.is_empty() {
                    "sort-by requires a column name"
                } else {
                    "sort-by takes one column name"
                };
                return Err(usage_error(problem, "sort-by [--desc] <column>"));
            }
        };
        let mut batch = input.into_batch();
        let Some(col_idx) = batch.col_index(&col) else {
            return Ok(Value::from_batch(batch));
        };
        let order = if desc {
            batch.order_by(|row| Reverse(SortKey::of(row.get(col_idx))))
        } else {
            batch.order_by(|row| SortKey::of(row.get(col_idx)))
        };
        batch.take_rows(order);
        Ok(Value::from_batch(batch))
    }
}

/// Numbers compare as numbers, everything else as text, with non-strings as the empty string.
/// Mixed columns therefore have no total order; the sort is stable over whatever it yields.
struct SortKey<'a> {
    num: Option<f64>,
    text: &'a str,
}

impl<'a> SortKey<'a> {
    fn of(cell: Datum<'a>) -> Self {
        Self {
            num: cell.as_f64(),
            text: cell.as_str().unwrap_or(""),
        }
    }
}

impl Ord for SortKey<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.num, other.num) {
            (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
            _ => self.text.cmp(other.text),
        }
    }
}

impl PartialOrd for SortKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for SortKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for SortKey<'_> {}
