use shell_kernel::Session;
use shell_value::{ColType, Datum, Record, RecordBatch, Rows, StructuredDiagnostic, Value, record};

use super::common::usage_error;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct SchemaCmd;

record! {
    struct SchemaRow<'a> {
        #[text] column: &'a str,
        kind: &'a str,
        non_null: u64,
        rows: u64,
    }
}

static SIGNATURE: Signature = Signature {
    name: "schema",
    aliases: &["describe"],
    description: "Inspect columns and types of a structured table",
    flags: &[],
    columns: <SchemaRow<'static> as Record>::COLUMNS,
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for SchemaCmd {
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
        if !parsed.operands.is_empty() {
            return Err(usage_error("schema takes no arguments", "schema"));
        }
        let batch = input.into_batch()?;
        if batch.ncols() == 0 {
            return Ok(Value::from_batch(RecordBatch::empty()));
        }

        let total = batch.nrows() as u64;
        let mut rows = Rows::<SchemaRow<'static>>::with_capacity(batch.ncols());
        for (col_idx, col) in batch.schema().columns().iter().enumerate() {
            let mut non_null = 0u64;
            let mut observed: Option<&'static str> = match col.ty() {
                ColType::Timestamp => Some("timestamp"),
                ColType::Mode => Some("mode"),
                _ => None,
            };
            let mut mixed = false;
            for row in batch.rows() {
                let d = row.get(col_idx);
                if d.is_null() {
                    continue;
                }
                non_null += 1;
                if col.ty() == ColType::Any {
                    let k = datum_kind(d);
                    match observed {
                        None => observed = Some(k),
                        Some(prev) if prev != k => mixed = true,
                        _ => {}
                    }
                }
            }
            let kind = if mixed {
                "mixed"
            } else {
                observed.unwrap_or("null")
            };
            rows.push(&SchemaRow {
                column: col.name(),
                kind,
                non_null,
                rows: total,
            });
        }
        Ok(Value::from_batch(rows.finish_or_empty()?))
    }
}

fn datum_kind(d: Datum<'_>) -> &'static str {
    match d {
        Datum::Null => "null",
        Datum::Bool(_) => "bool",
        Datum::Int(_) => "int",
        Datum::UInt(_) => "uint",
        Datum::Float(_) => "float",
        Datum::Str(_) => "str",
        Datum::Json(_) => "json",
        Datum::Time(_) => "timestamp",
        Datum::Mode(_) => "mode",
        _ => "any",
    }
}
