use std::fs;
use std::path::Path;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Mkdir;

static SIGNATURE: Signature = Signature {
    name: "mkdir",
    aliases: &[],
    description: "Create directories",
    flags: &[
        Flag::switch(
            'p',
            "parents",
            "create missing parents; existing directories are fine",
        ),
        Flag::switch('v', "verbose", "say what was created"),
    ],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Mkdir {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// Mirrors uutils `mkdir` (mkdir.rs): every operand is attempted and each failure reported.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        if args.operands.is_empty() {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "mkdir: missing operand",
                None,
                Some("usage: mkdir [-pv] DIRECTORY...".to_string()),
            ));
        }
        let parents = args.has("p");
        let verbose = args.has("v");
        let mut out = Vec::new();
        for operand in &args.operands {
            let path = resolve_path(session, operand);
            let missing: Vec<&Path> = path
                .ancestors()
                .take_while(|a| fs::symlink_metadata(a).is_err())
                .collect();
            if let Some(top) = missing.last() {
                session.record_preimage(top);
            }
            let created = if parents {
                fs::create_dir_all(&path)
            } else {
                fs::create_dir(&path)
            };
            match created {
                Ok(()) => {
                    session.record_mutation(path.to_string_lossy().to_string());
                    if verbose {
                        announce(&mut out, operand, &path, &missing);
                    }
                }
                Err(e) => {
                    fail(
                        session,
                        "E_MKDIR",
                        format!("cannot create directory '{operand}': {}", describe(&e)),
                    );
                }
            }
        }
        Ok(Value::from_bytes(out))
    }
}

/// One line per directory that was missing before, outermost first, named as the operand is.
fn announce(out: &mut Vec<u8>, operand: &str, path: &Path, missing: &[&Path]) {
    for created in missing.iter().rev() {
        let depth = path.components().count() - created.components().count();
        let mut label = operand.trim_end_matches('/');
        for _ in 0..depth {
            label = label.rsplit_once('/').map_or(label, |(parent, _)| parent);
        }
        out.extend_from_slice(format!("mkdir: created directory '{label}'\n").as_bytes());
    }
}
