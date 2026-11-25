use shell_kernel::Session;
use shell_value::{Column, Datum, DynRows, Schema, StructuredDiagnostic, Value};

use super::common::{text_lines, usage_error};
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Count;

const COLUMNS: &[Column] = &[Column::new("count")];

static SIGNATURE: Signature = Signature {
    name: "count",
    aliases: &["length"],
    description: "Count rows in a structured table or lines in byte input",
    flags: &[],
    columns: COLUMNS,
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Count {
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
            return Err(usage_error("count takes no arguments", "count"));
        }
        let n = match input {
            Value::Empty => 0u64,
            Value::Batch(batch) => batch.nrows() as u64,
            Value::Bytes(bytes) => text_lines(&bytes).count() as u64,
        };
        let mut rows = DynRows::new(Schema::of(COLUMNS));
        rows.push_with(|out| out.put(Datum::UInt(n)));
        Ok(Value::from_batch(rows.finish()?))
    }
}
