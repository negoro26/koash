use std::borrow::Cow;

use shell_kernel::{Session, find_executable};
use shell_value::{Record, Rows, StructuredDiagnostic, Value, record};

use super::common::usage_error;
use super::lookup;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

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
    flags: &[Flag::short_switch(
        't',
        "print only the kind: alias, builtin or file",
    )],
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
        let kinds_only = parsed.has("t");
        let mut kinds = String::new();
        let mut rows = Rows::<TypeRow<'static>>::with_capacity(parsed.operands.len());
        for &name in &parsed.operands {
            let found: Option<(&str, Cow<str>)> = if let Some(val) = session.aliases.get(name) {
                Some(("alias", Cow::Borrowed(val.as_str())))
            } else if let Some(b) = lookup(name) {
                Some(("builtin", Cow::Borrowed(b.signature().description)))
            } else {
                find_executable(session, name)
                    .map(|path| ("external", Cow::Owned(path.to_string_lossy().into_owned())))
            };
            match found {
                Some((kind, _)) if kinds_only => {
                    kinds.push_str(if kind == "external" { "file" } else { kind });
                    kinds.push('\n');
                }
                Some((kind, detail)) => rows.push(&TypeRow {
                    name,
                    kind,
                    detail: &detail,
                }),
                None => session.report(StructuredDiagnostic::new(
                    "E_NOT_FOUND",
                    format!("type: {name}: not found"),
                    None,
                    None,
                )),
            }
        }
        if kinds_only {
            return Ok(Value::from_bytes(kinds.into_bytes()));
        }
        Ok(Value::from_batch(rows.finish_or_empty()?))
    }
}
