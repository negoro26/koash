use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Echo;

static SIGNATURE: Signature = Signature {
    name: "echo",
    aliases: &[],
    description: "Print arguments as raw text",
    flags: &[Flag::short_switch('n', "do not print the trailing newline")],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Echo {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// POSIX `echo`: `-n` counts only before the first word, and any other dash word,
    /// `--` included, is printed as text.
    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse_leading(&SIGNATURE, args)?;
        let mut out = args.operands.join(" ").into_bytes();
        if !args.has("n") {
            out.push(b'\n');
        }
        Ok(Value::from_bytes(out))
    }
}
