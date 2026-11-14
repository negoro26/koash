use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::{format_unix_iso, resolve_path};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Ls;

static SIGNATURE: Signature = Signature {
    name: "ls",
    aliases: &[],
    description: "List directory entries as structured records",
    flags: &[
        Flag::switch('a', "all", "include entries whose name starts with a dot"),
        Flag::switch(
            'A',
            "almost-all",
            "same as -a; `.` and `..` are never listed",
        ),
        Flag::short_switch(
            'l',
            "long listing; every record already carries size, time, and mode",
        ),
    ],
    columns: crate::builtin::columns!["name", "ext", "kind", "size", "modified", "permissions"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Ls {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    /// A directory operand is listed, and so is a symlink that points at one (uutils ls.rs:848-851,
    /// `DirArgs`). Every operand is attempted; an unreadable one is reported, not dropped.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let show_all = args.has("all") || args.has("almost-all");
        let targets: Vec<&str> = if args.operands.is_empty() {
            vec!["."]
        } else {
            args.operands.clone()
        };

        let mut records = Vec::new();
        for target in targets {
            let resolved = resolve_path(session, target);
            if std::fs::metadata(&resolved).is_ok_and(|m| m.is_dir()) {
                list_dir(session, &resolved, target, show_all, &mut records);
                continue;
            }
            match std::fs::symlink_metadata(&resolved) {
                Ok(meta) => records.push(build_ls_record(target.to_string(), &resolved, &meta)),
                Err(e) => {
                    fail(
                        session,
                        "E_LS",
                        format!("cannot access '{target}': {}", describe(&e)),
                    );
                }
            }
        }

        records.sort_by(|a, b| {
            let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
            na.cmp(nb)
        });

        Value::from_records(records)
    }
}

fn list_dir(
    session: &mut Session,
    dir: &Path,
    label: &str,
    show_all: bool,
    records: &mut Vec<StructuredRecord>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            fail(
                session,
                "E_LS",
                format!("cannot open directory '{label}': {}", describe(&e)),
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                fail(
                    session,
                    "E_LS",
                    format!("cannot read directory '{label}': {}", describe(&e)),
                );
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_all && name.starts_with('.') {
            continue;
        }
        match entry.metadata() {
            Ok(meta) => records.push(build_ls_record(name, &entry.path(), &meta)),
            Err(e) => {
                fail(
                    session,
                    "E_LS",
                    format!("cannot access '{label}/{name}': {}", describe(&e)),
                );
            }
        }
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
