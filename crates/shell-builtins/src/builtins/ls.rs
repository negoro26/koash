use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use shell_kernel::Session;
use shell_value::{
    FileMode, OrBlank, Record, Rows, StructuredDiagnostic, UnixTime, Value, column_index, record,
};

use super::common::{kind_of, resolve_path};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

record! {
    struct LsRow<'a> {
        ext: &'a str,
        kind: &'a str,
        modified: OrBlank<UnixTime>,
        name: &'a str,
        permissions: FileMode,
        size: u64,
    }
}

const NAME: usize = column_index(<LsRow<'static> as Record>::COLUMNS, "name");

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

        let mut rows = Rows::<LsRow<'static>>::new();
        for target in targets {
            let resolved = resolve_path(session, target);
            if std::fs::metadata(&resolved).is_ok_and(|m| m.is_dir()) {
                list_dir(session, &resolved, target, show_all, &mut rows);
                continue;
            }
            match std::fs::symlink_metadata(&resolved) {
                Ok(meta) => push_ls_row(&mut rows, target, &resolved, &meta),
                Err(e) => {
                    fail(
                        session,
                        "E_LS",
                        format!("cannot access '{target}': {}", describe(&e)),
                    );
                }
            }
        }

        let mut batch = rows.finish_or_empty()?;
        let order = batch.order_by(|row| row.get(NAME).as_str().unwrap_or(""));
        batch.take_rows(order);
        Ok(Value::from_batch(batch))
    }
}

fn list_dir(
    session: &mut Session,
    dir: &Path,
    label: &str,
    show_all: bool,
    rows: &mut Rows<LsRow<'static>>,
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
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if !show_all && name.starts_with('.') {
            continue;
        }
        match entry.metadata() {
            Ok(meta) => push_ls_row(rows, &name, Path::new(&file_name), &meta),
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

/// `path` supplies the extension; `name` is what the row calls the entry.
fn push_ls_row(rows: &mut Rows<LsRow<'static>>, name: &str, path: &Path, meta: &std::fs::Metadata) {
    rows.push(&LsRow {
        ext: path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        kind: kind_of(meta),
        modified: OrBlank(UnixTime::from_system(meta.modified())),
        name,
        permissions: FileMode(meta.permissions().mode() & 0o777),
        size: meta.len(),
    });
}
