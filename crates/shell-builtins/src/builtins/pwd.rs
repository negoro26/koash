use serde_json::Value as JsonVal;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Pwd;

static SIGNATURE: Signature = Signature {
    name: "pwd",
    aliases: &[],
    description: "Print current working directory",
    flags: &[
        Flag::short_switch(
            'L',
            "print the logical directory, symlinks as entered (default)",
        ),
        Flag::short_switch('P', "print the physical directory, symlinks resolved"),
    ],
    columns: &["path"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Pwd {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// `session.cwd` is the logical directory: `cd` stores it as typed and never resolves symlinks.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let path = if args.last_of(&["L", "P"]) == Some("P") {
            session
                .cwd
                .canonicalize()
                .map_err(|e| StructuredDiagnostic::from_io("E_PWD", &e, None))?
        } else {
            session.cwd.clone()
        };
        let mut rec = StructuredRecord::new();
        rec.insert(
            "path".to_string(),
            JsonVal::String(path.to_string_lossy().to_string()),
        );
        Ok(Value::from_records(vec![rec]))
    }
}
