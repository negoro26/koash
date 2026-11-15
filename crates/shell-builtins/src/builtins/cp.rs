use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::fs_ops::{CopyOpts, copy_operand, plan_transfers};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Cp;

static SIGNATURE: Signature = Signature {
    name: "cp",
    aliases: &[],
    description: "Copy files or directories",
    flags: &[
        Flag::switch('r', "recursive", "copy directories and their contents"),
        Flag::short_switch('R', "same as -r"),
        Flag::switch('n', "no-clobber", "never replace an existing file"),
        Flag::switch('f', "force", "remove an unwritable target and retry"),
        Flag::switch('p', "preserve", "keep mode and modification time"),
        Flag::switch('L', "dereference", "follow every symlink in the source"),
        Flag::switch('P', "no-dereference", "copy symlinks as symlinks"),
        Flag::switch('v', "verbose", "say what was copied"),
    ],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Cp {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// Mirrors uutils `cp` (cp.rs): `cp SRC... DIR` and `cp SRC DST`, symlinks copied as symlinks
    /// under `-r` unless `-L`, and an existing target replaced unless `-n`.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let transfers = plan_transfers(session, "cp", "E_CP", &args.operands)?;
        let recursive = args.has("r") || args.has("R");
        let deref = args.last_of(&["L", "P"]);
        let opts = CopyOpts {
            recursive,
            no_clobber: args.last_of(&["n", "f"]) == Some("n"),
            force: args.last_of(&["n", "f"]) == Some("f"),
            preserve: args.has("p"),
            dereference: deref == Some("L"),
            follow_operands: deref == Some("L") || (deref.is_none() && !recursive),
            verbose: args.has("v"),
        };
        let mut out = Vec::new();
        for t in &transfers {
            copy_operand(
                session,
                "E_CP",
                &t.src,
                &t.dest,
                (&t.src_label, &t.dest_label),
                opts,
                &mut out,
            );
        }
        Ok(Value::from_bytes(out))
    }
}
