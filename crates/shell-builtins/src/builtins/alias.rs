use std::fmt::Write;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::fs_ops::fail;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Alias;

static SIGNATURE: Signature = Signature {
    name: "alias",
    aliases: &[],
    description: "Define or list command aliases for this session",
    flags: &[Flag::short_switch(
        'p',
        "print aliases in a form that can be reused as input",
    )],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: false,
};

const ILLEGAL_NAME_CHARS: &str = "()<>;&| \t\n\"'`\\$/";

fn is_legal_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(|c| ILLEGAL_NAME_CHARS.contains(c))
}

fn push_definition(out: &mut String, name: &str, value: &str) {
    let separator = if name.starts_with('-') { "-- " } else { "" };
    let _ = writeln!(
        out,
        "alias {separator}{name}='{}'",
        value.replace('\'', "'\\''")
    );
}

impl Builtin for Alias {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let mut out = String::new();
        if args.operands.is_empty() {
            for (name, value) in &session.aliases {
                push_definition(&mut out, name, value);
            }
        }
        for operand in &args.operands {
            match operand.split_once('=') {
                Some((name, value)) if !name.is_empty() => {
                    if is_legal_name(name) {
                        session.aliases.insert(name.to_string(), value.to_string());
                    } else {
                        fail(
                            session,
                            "E_ALIAS",
                            format!("alias: `{name}': invalid alias name"),
                        );
                    }
                }
                _ => match session.aliases.get(*operand) {
                    Some(value) => push_definition(&mut out, operand, value),
                    None => {
                        fail(session, "E_ALIAS", format!("alias: {operand}: not found"));
                    }
                },
            }
        }
        Ok(if out.is_empty() {
            Value::Empty
        } else {
            Value::Bytes(out.into_bytes())
        })
    }
}
