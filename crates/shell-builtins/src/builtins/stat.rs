use std::os::unix::fs::PermissionsExt;

use shell_kernel::Session;
use shell_value::{FileMode, OrBlank, Rows, StructuredDiagnostic, UnixTime, Value, record};

use super::common::{kind_of, resolve_path, usage_error};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

record! {
    struct StatRow<'a> {
        kind: &'a str,
        mode: FileMode,
        modified: OrBlank<UnixTime>,
        path: &'a str,
        readonly: bool,
        size: u64,
    }
}

pub(super) struct Stat;

static SIGNATURE: Signature = Signature {
    name: "stat",
    aliases: &[],
    description: "Inspect file or directory metadata",
    flags: &[],
    columns: crate::builtin::columns!["path", "kind", "size", "modified", "readonly", "mode"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Stat {
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
        if args.operands.is_empty() {
            return Err(usage_error(
                "stat requires at least one path",
                "stat <path>...",
            ));
        }

        let mut rows = Rows::<StatRow<'static>>::new();
        for arg in &args.operands {
            let p = resolve_path(session, arg);
            let meta = match std::fs::symlink_metadata(&p) {
                Ok(meta) => meta,
                Err(e) => {
                    fail(
                        session,
                        "E_STAT",
                        format!("cannot stat '{arg}': {}", describe(&e)),
                    );
                    continue;
                }
            };
            let perms = meta.permissions();
            rows.push(&StatRow {
                kind: kind_of(&meta),
                mode: FileMode(perms.mode() & 0o777),
                modified: OrBlank(UnixTime::from_system(meta.modified())),
                path: arg,
                readonly: perms.readonly(),
                size: meta.len(),
            });
        }
        Ok(Value::from_batch(rows.finish_or_empty()?))
    }
}
