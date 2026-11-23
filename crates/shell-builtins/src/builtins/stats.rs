use shell_kernel::Session;
use shell_value::{Column, Datum, DynRows, RecordBatch, Schema, StructuredDiagnostic, Value};

use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Stats;

const COLUMNS: &[Column] = &[
    Column::new("column").text(),
    Column::new("count"),
    Column::new("sum"),
    Column::new("min"),
    Column::new("max"),
    Column::new("avg"),
];

static SIGNATURE: Signature = Signature {
    name: "stats",
    aliases: &["measure"],
    description: "Compute count, sum, min, max, and avg on numeric columns",
    flags: &[],
    columns: COLUMNS,
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Stats {
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
        let batch = input.into_batch()?;
        if batch.is_empty() {
            return Ok(Value::from_batch(RecordBatch::empty()));
        }

        let target_cols: Vec<usize> = if parsed.operands.is_empty() {
            (0..batch.ncols()).collect()
        } else {
            parsed
                .operands
                .iter()
                .filter_map(|name| batch.col_index(name))
                .collect()
        };

        let mut out = DynRows::new(Schema::of(COLUMNS));
        let mut any = false;

        for col_idx in target_cols {
            let mut count = 0u64;
            let mut sum = 0.0f64;
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;

            for row in batch.rows() {
                let Some(v) = row.get(col_idx).as_f64() else {
                    continue;
                };
                if !v.is_finite() {
                    continue;
                }
                count += 1;
                sum += v;
                if v < min {
                    min = v;
                }
                if v > max {
                    max = v;
                }
            }

            if count == 0 {
                continue;
            }
            any = true;
            let avg = sum / (count as f64);
            let name = batch.schema().columns()[col_idx].name();
            out.push_with(|sink| {
                sink.put_str(name);
                sink.put(Datum::UInt(count));
                sink.put(clean_num(sum));
                sink.put(clean_num(min));
                sink.put(clean_num(max));
                sink.put(Datum::Float((avg * 1000.0).round() / 1000.0));
            });
        }

        if !any {
            return Ok(Value::from_batch(RecordBatch::empty()));
        }
        Ok(Value::from_batch(out.finish()?))
    }
}

fn clean_num(v: f64) -> Datum<'static> {
    if v.fract() == 0.0 && v >= 0.0 && v <= (u64::MAX as f64) {
        Datum::UInt(v as u64)
    } else if v.fract() == 0.0 && v >= (i64::MIN as f64) && v <= (i64::MAX as f64) {
        Datum::Int(v as i64)
    } else {
        Datum::Float(v)
    }
}
