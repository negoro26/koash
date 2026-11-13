use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

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

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        for arg in args {
            if let Some((k, v)) = arg.split_once('=') {
                session.env.insert(k.to_string(), v.to_string());
            }
        }
        Ok(Value::Empty)
    }
}
