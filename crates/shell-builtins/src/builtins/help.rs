use std::borrow::Cow;
use std::sync::Arc;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::catalog;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy, flag_spellings};

pub(super) struct Help;

static SIGNATURE: Signature = Signature {
    name: "help",
    aliases: &[],
    description: "Show builtin commands and flags",
    flags: &[],
    columns: &["command", "description", "flags", "columns"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
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
        let cols: Arc<[Cow<'static, str>]> = Arc::new([
            Cow::Borrowed("command"),
            Cow::Borrowed("flags"),
            Cow::Borrowed("columns"),
            Cow::Borrowed("description"),
        ]);
        let filter = args.first().map(|s| s.as_str());
        let mut batch = shell_value::RecordBatch::with_capacity(cols, catalog().count());
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
            batch
                .cells
                .push(shell_value::CellVal::Str(schema.columns.join(", ")));
            batch
                .cells
                .push(shell_value::CellVal::Str(schema.description.to_string()));
        }
        Ok(Value::from_batch(batch))
    }
}
