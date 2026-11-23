use shell_kernel::{Session, find_executable};
use shell_value::{Record, Rows, StructuredDiagnostic, Value, record};

use super::common::usage_error;
use super::lookup;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct TypeCmd;

record! {
    struct TypeRow<'a> {
        name: &'a str,
        kind: &'a str,
        #[text] detail: &'a str,
    }
}

static SIGNATURE: Signature = Signature {
    name: "type",
    aliases: &[],
    description: "Describe how each command name would be interpreted (alias, builtin, external)",
    flags: &[],
    columns: <TypeRow<'static> as Record>::COLUMNS,
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: true,
};

impl Builtin for TypeCmd {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&SIGNATURE, args)?;
        if parsed.operands.is_empty() {
            return Err(usage_error("type requires a command name", "type NAME..."));
        }
        let mut rows = Rows::<TypeRow<'static>>::with_capacity(parsed.operands.len());
        for &name in &parsed.operands {
            if let Some(val) = session.aliases.get(name) {
                rows.push(&TypeRow {
                    name,
                    kind: "alias",
                    detail: val,
                });
            } else if let Some(b) = lookup(name) {
                rows.push(&TypeRow {
                    name,
                    kind: "builtin",
                    detail: b.signature().description,
                });
            } else if let Some(path) = find_executable(session, name) {
                let text = path.to_string_lossy();
                rows.push(&TypeRow {
                    name,
                    kind: "external",
                    detail: &text,
                });
            } else {
                session.report(StructuredDiagnostic::new(
                    "E_NOT_FOUND",
                    format!("type: {name}: not found"),
                    None,
                    None,
                ));
            }
        }
        Ok(Value::from_batch(rows.finish_or_empty()?))
    }
}
