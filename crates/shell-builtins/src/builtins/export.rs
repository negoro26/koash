use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::fs_ops::fail;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Export;

static SIGNATURE: Signature = Signature {
    name: "export",
    aliases: &[],
    description: "Export environment variables",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Export {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// `NAME=VALUE` sets the variable; a bare `NAME` exports one that exists. The session keeps
    /// no unexported variables, so a bare name needs no work, only a valid identifier.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        for operand in &args.operands {
            let (name, value) = match operand.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (*operand, None),
            };
            if !is_identifier(name) {
                fail(
                    session,
                    "E_EXPORT",
                    format!("export: '{operand}': not a valid identifier"),
                );
                continue;
            }
            if let Some(value) = value {
                session.env.insert(name.to_string(), value.to_string());
            }
        }
        Ok(Value::Empty)
    }
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}
