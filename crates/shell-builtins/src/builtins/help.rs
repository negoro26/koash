use shell_kernel::Session;
use shell_value::{Column, RecordBatch, Schema, StructuredDiagnostic, Value};

use super::catalog;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy, flag_spellings};

pub(super) struct Help;

static COLUMNS: [Column; 4] = [
    Column::new("command"),
    Column::new("flags"),
    Column::new("columns"),
    Column::new("description"),
];

static SIGNATURE: Signature = Signature {
    name: "help",
    aliases: &[],
    description: "Show builtin commands and flags",
    flags: &[],
    columns: crate::builtin::columns!["command", "description", "flags", "columns"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Help {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let filter = args.first().map(|s| s.as_str());
        let mut batch = RecordBatch::with_capacity(Schema::of(&COLUMNS), catalog().count());
        for schema in catalog() {
            if let Some(q) = filter
                && !schema.name.eq_ignore_ascii_case(q)
                && !schema.name.contains(q)
                && !schema.description.contains(q)
            {
                continue;
            }
            batch
                .cells
                .push(shell_value::CellVal::Str(schema.name.to_string()));
            batch.cells.push(shell_value::CellVal::Str(
                flag_spellings(schema.flags).join(" "),
            ));
            batch.cells.push(shell_value::CellVal::Str(
                schema.column_names().collect::<Vec<_>>().join(", "),
            ));
            batch
                .cells
                .push(shell_value::CellVal::Str(schema.description.to_string()));
        }
        Ok(Value::from_batch(batch))
    }
}
