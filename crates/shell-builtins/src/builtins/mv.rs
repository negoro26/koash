use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::fs_ops::{MoveOpts, move_operand, plan_transfers};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Mv;

static SIGNATURE: Signature = Signature {
    name: "mv",
    aliases: &[],
    description: "Move or rename files",
    flags: &[
        Flag::switch('n', "no-clobber", "never replace an existing file"),
        Flag::switch(
            'f',
            "force",
            "replace existing files without asking (the default)",
        ),
        Flag::switch('v', "verbose", "say what was moved"),
    ],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Mv {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// Mirrors uutils `mv` (mv.rs): `mv SRC... DIR` and `mv SRC DST`, a cross-device rename
    /// falls back to copy and remove, and a directory never moves into itself.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let transfers = plan_transfers(session, "mv", "E_MV", &args.operands)?;
        let opts = MoveOpts {
            no_clobber: args.last_of(&["n", "f"]) == Some("n"),
            verbose: args.has("v"),
        };
        let mut out = Vec::new();
        for t in &transfers {
            move_operand(session, "E_MV", t, opts, &mut out);
        }
        Ok(Value::from_bytes(out))
    }
}
