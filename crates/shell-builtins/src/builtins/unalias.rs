use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::usage_error;
use super::fs_ops::fail;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Unalias;

static SIGNATURE: Signature = Signature {
    name: "unalias",
    aliases: &[],
    description: "Remove command aliases from this session",
    flags: &[Flag::short_switch('a', "remove every alias")],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: false,
};

impl Builtin for Unalias {
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
        if args.has("a") {
            session.aliases.clear();
            return Ok(Value::Empty);
        }
        if args.operands.is_empty() {
            return Err(usage_error(
                "unalias: expected an alias name or -a",
                "unalias [-a] name [name ...]",
            ));
        }
        for name in &args.operands {
            if session.aliases.remove(*name).is_none() {
                fail(session, "E_ALIAS", format!("unalias: {name}: not found"));
            }
        }
        Ok(Value::Empty)
    }
}
