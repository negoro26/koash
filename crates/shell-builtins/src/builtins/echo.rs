use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Echo;

static SIGNATURE: Signature = Signature {
    name: "echo",
    aliases: &[],
    description: "Print arguments as raw text",
    flags: &["-n"],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Echo {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut no_newline = false;
        let mut parts: Vec<&str> = Vec::new();
        let mut parse_flags = true;
        for arg in args {
            if parse_flags && arg == "-n" {
                no_newline = true;
            } else {
                parse_flags = false;
                parts.push(arg.as_str());
            }
        }
        let mut out = parts.join(" ").into_bytes();
        if !no_newline {
            out.push(b'\n');
        }
        Ok(Value::from_bytes(out))
    }
}
