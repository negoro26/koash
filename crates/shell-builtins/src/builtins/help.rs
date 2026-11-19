use shell_kernel::Session;
use shell_value::{Rows, StructuredDiagnostic, Value, record};

use super::catalog;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy, flag_spellings};

pub(super) struct Help;

record! {
    struct HelpRow<'a> {
        command: &'a str,
        #[detail]
        flags: &'a str,
        columns: &'a str,
        description: &'a str,
    }
}

static SIGNATURE: Signature = Signature {
    name: "help",
    aliases: &[],
    description: "Show builtin commands and flags; other options run the system command",
    flags: &[],
    columns: crate::builtin::columns!["command", "description", "flags", "columns"],
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
        let filter = args.first().map(|s| s.as_str());
        let exact = filter
            .filter(|q| catalog().any(|s| s.name.eq_ignore_ascii_case(q) || s.aliases.contains(q)));
        let mut rows = Rows::<HelpRow<'static>>::with_capacity(catalog().count());
        for schema in catalog() {
            if let Some(q) = exact {
                if !schema.name.eq_ignore_ascii_case(q) && !schema.aliases.contains(&q) {
                    continue;
                }
            } else if let Some(q) = filter
                && !schema.name.contains(q)
                && !schema.description.contains(q)
            {
                continue;
            }
            rows.push(&HelpRow {
                command: schema.name,
                flags: &flag_spellings(schema.flags).join(" "),
                columns: &schema.column_names().collect::<Vec<_>>().join(", "),
                description: schema.description,
            });
        }
        Ok(Value::from_batch(rows.finish()?))
    }
}
