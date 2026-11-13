use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::{format_unix_iso, resolve_path};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Ls;

static SIGNATURE: Signature = Signature {
    name: "ls",
    aliases: &[],
    description: "List directory entries as structured records",
    flags: &["-a", "-l"],
    columns: &["name", "ext", "kind", "size", "modified", "permissions"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Ls {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut show_all = false;
        let mut targets: Vec<&str> = Vec::new();
        let mut parse_flags = true;

        for arg in args {
            if parse_flags && arg == "--" {
                parse_flags = false;
            } else if parse_flags && arg.starts_with('-') && arg.len() > 1 {
                if arg.contains('a') {
                    show_all = true;
                }
            } else {
                targets.push(arg.as_str());
            }
        }

        if targets.is_empty() {
            targets.push(".");
        }

        let mut records = Vec::new();
        for target in targets {
            let resolved = if target == "." {
                session.cwd.clone()
            } else {
                resolve_path(session, target)
            };
            let meta = std::fs::symlink_metadata(&resolved)
                .map_err(|e| StructuredDiagnostic::from_io("E_LS", &e, None))?;
            if !meta.is_dir() {
                records.push(build_ls_record(target.to_string(), &resolved, &meta));
                continue;
            }

            let entries = std::fs::read_dir(&resolved)
                .map_err(|e| StructuredDiagnostic::from_io("E_LS", &e, None))?;
            for entry_res in entries {
                let Ok(entry) = entry_res else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().to_string();
                if !show_all && name.starts_with('.') {
                    continue;
                }
                let Ok(entry_meta) = entry.metadata() else {
                    continue;
                };
                records.push(build_ls_record(name, &entry.path(), &entry_meta));
            }
        }

        records.sort_by(|a, b| {
            let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
            na.cmp(nb)
        });

        Ok(Value::from_records(records))
    }
}

fn build_ls_record(name: String, path: &Path, meta: &std::fs::Metadata) -> StructuredRecord {
    let kind = if meta.is_dir() {
        "dir"
    } else if meta.is_symlink() {
        "symlink"
    } else {
        "file"
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| format_unix_iso(d.as_secs()))
        .unwrap_or_default();
    let perms = format!("{:o}", meta.permissions().mode() & 0o777);

    let mut rec = StructuredRecord::new();
    rec.insert("name".to_string(), JsonVal::String(name));
    rec.insert("ext".to_string(), JsonVal::String(ext));
    rec.insert("kind".to_string(), JsonVal::String(kind.to_string()));
    rec.insert("size".to_string(), json!(meta.len()));
    rec.insert("modified".to_string(), JsonVal::String(modified));
    rec.insert("permissions".to_string(), JsonVal::String(perms));
    rec
}
