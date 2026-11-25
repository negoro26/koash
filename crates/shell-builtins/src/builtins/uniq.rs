use std::collections::HashMap;

use shell_kernel::Session;
use shell_value::{Column, Datum, DynRows, RecordBatch, Schema, StructuredDiagnostic, Value};

use super::common::usage_error;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Uniq;

const COUNT_NAMES: [&str; 2] = ["count", "uniq_count"];

static SIGNATURE: Signature = Signature {
    name: "uniq",
    aliases: &[],
    description: "Deduplicate table rows by whole row or by column; text runs GNU uniq",
    flags: &[Flag::switch(
        'c',
        "count",
        "include occurrence count per unique row",
    )],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Uniq {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn text_writes(&self, args: &[String]) -> Option<Vec<String>> {
        let operands = Args::parse(&SIGNATURE, args).ok()?.operands;
        Some(
            operands
                .get(1)
                .filter(|out| **out != "-")
                .map(|out| (*out).to_string())
                .into_iter()
                .collect(),
        )
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&SIGNATURE, args)?;
        let with_count = parsed.has("count");
        let col_name = match parsed.operands.as_slice() {
            [] => None,
            [col] => Some(*col),
            _ => {
                return Err(usage_error(
                    "uniq takes at most one column name",
                    "uniq [-c|--count] [column]",
                ));
            }
        };

        let batch = input.into_batch()?;
        if batch.is_empty() {
            return Ok(Value::from_batch(RecordBatch::empty()));
        }

        let key_col = match col_name {
            Some(name) => match batch.col_index(name) {
                Some(idx) => Some(idx),
                None => return Ok(Value::from_batch(RecordBatch::empty())),
            },
            None => None,
        };

        let mut index_by_key: HashMap<String, usize> = HashMap::new();
        let mut unique_rows: Vec<(usize, u64)> = Vec::new();
        let mut scratch = String::new();
        let mut key_buf = String::new();

        for (row_idx, row) in batch.rows().enumerate() {
            key_buf.clear();
            match key_col {
                Some(col) => {
                    key_buf.push_str(row.get(col).text(&mut scratch));
                }
                None => {
                    for (c, datum) in row.iter().enumerate() {
                        if c > 0 {
                            key_buf.push('\x1f');
                        }
                        key_buf.push_str(datum.text(&mut scratch));
                    }
                }
            }
            if let Some(&slot) = index_by_key.get(&key_buf) {
                unique_rows[slot].1 += 1;
            } else {
                let slot = unique_rows.len();
                index_by_key.insert(key_buf.clone(), slot);
                unique_rows.push((row_idx, 1));
            }
        }

        if !with_count {
            let mut out = DynRows::new(batch.schema().clone());
            for (row_idx, _) in unique_rows {
                let row = batch.row(row_idx);
                out.push_with(|sink| {
                    for datum in row.iter() {
                        sink.put(datum);
                    }
                });
            }
            return Ok(Value::from_batch(out.finish()?));
        }

        let Some(count_name) = COUNT_NAMES
            .into_iter()
            .find(|name| batch.col_index(name).is_none())
        else {
            return Err(usage_error(
                "uniq -c needs a free column named count or uniq_count",
                "uniq [-c|--count] [column]",
            ));
        };
        let mut cols: Vec<Column> = batch.schema().columns().to_vec();
        cols.push(Column::new(count_name));
        let mut out = DynRows::new(Schema::from(cols));
        for (row_idx, count) in unique_rows {
            let row = batch.row(row_idx);
            out.push_with(|sink| {
                for datum in row.iter() {
                    sink.put(datum);
                }
                sink.put(Datum::UInt(count));
            });
        }
        Ok(Value::from_batch(out.finish()?))
    }
}
