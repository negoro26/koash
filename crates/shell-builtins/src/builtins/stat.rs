use std::os::unix::fs::PermissionsExt;
use std::time::UNIX_EPOCH;

use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::{format_unix_iso, resolve_path, usage_error};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

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

        let mut records = Vec::new();
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
            let kind = if meta.is_dir() {
                "dir"
            } else if meta.is_symlink() {
                "symlink"
            } else {
                "file"
            };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| format_unix_iso(d.as_secs()))
                .unwrap_or_default();
            let perms = meta.permissions();

            let mut rec = StructuredRecord::new();
            rec.insert("path".to_string(), JsonVal::String((*arg).to_string()));
            rec.insert("kind".to_string(), JsonVal::String(kind.to_string()));
            rec.insert("size".to_string(), json!(meta.len()));
            rec.insert("modified".to_string(), JsonVal::String(modified));
            rec.insert("readonly".to_string(), json!(perms.readonly()));
            rec.insert(
                "mode".to_string(),
                JsonVal::String(format!("{:o}", perms.mode() & 0o777)),
            );
            records.push(rec);
        }
        Ok(Value::from_records(records))
    }
}
