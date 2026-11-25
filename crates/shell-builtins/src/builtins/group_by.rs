use std::cmp::Reverse;
use std::collections::HashMap;

use shell_kernel::Session;
use shell_value::{Record, RecordBatch, Rows, StructuredDiagnostic, Value, record};

use super::common::usage_error;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct GroupBy;

record! {
    struct GroupRow<'a> {
        #[text] group: &'a str,
        count: u64,
    }
}

static SIGNATURE: Signature = Signature {
    name: "group-by",
    aliases: &["group_by"],
    description: "Group structured records by column and count each group",
    flags: &[Flag::switch('a', "asc", "sort groups by count ascending")],
    columns: <GroupRow<'static> as Record>::COLUMNS,
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for GroupBy {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&SIGNATURE, args)?;
        let asc = parsed.has("asc");
        let col = match parsed.operands.as_slice() {
            [col] => *col,
            _ => {
                return Err(usage_error(
                    "group-by requires one column name",
                    "group-by [--asc] <column>",
                ));
            }
        };

        let batch = input.into_batch()?;
        let Some(col_idx) = batch.col_index(col) else {
            return Ok(Value::from_batch(RecordBatch::empty()));
        };

        let mut counts: HashMap<String, u64> = HashMap::new();
        let mut scratch = String::new();
        for row in batch.rows() {
            let key = row.get(col_idx).text(&mut scratch);
            if let Some(entry) = counts.get_mut(key) {
                *entry += 1;
            } else {
                counts.insert(key.to_string(), 1);
            }
        }

        let mut groups: Vec<(String, u64)> = counts.into_iter().collect();
        if asc {
            groups.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        } else {
            groups.sort_by(|a, b| Reverse(a.1).cmp(&Reverse(b.1)).then_with(|| a.0.cmp(&b.0)));
        }

        let mut rows = Rows::<GroupRow<'static>>::with_capacity(groups.len());
        for (group, count) in &groups {
            rows.push(&GroupRow {
                group: group.as_str(),
                count: *count,
            });
        }
        Ok(Value::from_batch(rows.finish_or_empty()?))
    }
}
