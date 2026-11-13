use std::os::unix::fs::PermissionsExt;
use std::time::UNIX_EPOCH;

use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::{format_unix_iso, resolve_path};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Stat;

static SIGNATURE: Signature = Signature {
    name: "stat",
    aliases: &[],
    description: "Inspect file or directory metadata",
    flags: &[],
    columns: &["path", "kind", "size", "modified", "readonly", "mode"],
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
        if args.is_empty() {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "stat requires at least one path",
                None,
                Some("usage: stat <path>...".to_string()),
            ));
        }

        let mut records = Vec::new();
        for arg in args {
            let p = resolve_path(session, arg);
            let meta = std::fs::symlink_metadata(&p)
                .map_err(|e| StructuredDiagnostic::from_io("E_STAT", &e, None))?;
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
            rec.insert("path".to_string(), JsonVal::String(arg.clone()));
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
