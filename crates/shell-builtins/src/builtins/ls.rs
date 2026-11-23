use std::fs::Metadata;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
        #[text] name: &'a str,
        permissions: FileMode,
        size: u64,
    }
}

const NAME: usize = column_index(<LsRow<'static> as Record>::COLUMNS, "name");

#[derive(Clone, Copy)]
enum SortBy {
    Name,
    Time,
    Size,
}

impl SortBy {
    fn key(self, meta: &Metadata) -> i128 {
        match self {
            Self::Name => 0,
            Self::Time => {
                -(i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()))
            }
            Self::Size => -i128::from(meta.len()),
        }
    }
}

struct Listing {
    rows: Rows<LsRow<'static>>,
    keys: Vec<i128>,
    sort: SortBy,
}

impl Listing {
    fn push(&mut self, name: &str, path: &Path, meta: &Metadata) {
        self.keys.push(self.sort.key(meta));
        self.rows.push(&LsRow {
            ext: path.extension().and_then(|e| e.to_str()).unwrap_or(""),
            kind: kind_of(meta),
            modified: OrBlank(UnixTime::from_system(meta.modified())),
            name,
            permissions: FileMode(meta.permissions().mode() & 0o7777),
            size: meta.len(),
        });
    }
}

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
        Flag::short_switch('1', "one entry per line; every record is already one row"),
        Flag::short_switch('t', "newest modification time first, ties by name"),
        Flag::short_switch('S', "largest size first, ties by name"),
        Flag::switch('r', "reverse", "reverse the order"),
        Flag::switch(
            'h',
            "human-readable",
            "accepted; size stays a number of bytes so it can be sorted and compared",
        ),
    ],
    columns: crate::builtin::columns!["name", "ext", "kind", "size", "modified", "permissions"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Ls {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn terminal_view(
        &self,
        session: &mut Session,
        args: &[String],
        columns: Option<&str>,
    ) -> Option<Result<Vec<u8>, StructuredDiagnostic>> {
        Some(super::ls_view::render(&SIGNATURE, session, args, columns))
    }

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

        let sort = match args.last_of(&["t", "S"]) {
            Some("t") => SortBy::Time,
            Some("S") => SortBy::Size,
            _ => SortBy::Name,
        };
        let mut listing = Listing {
            rows: Rows::new(),
            keys: Vec::new(),
            sort,
        };
        for target in targets {
            let resolved = resolve_path(session, target);
            if std::fs::metadata(&resolved).is_ok_and(|m| m.is_dir()) {
                list_dir(session, &resolved, target, show_all, &mut listing);
                continue;
            }
            match std::fs::symlink_metadata(&resolved) {
                Ok(meta) => listing.push(target, &resolved, &meta),
                Err(e) => {
                    fail(
                        session,
                        "E_LS",
                        format!("cannot access '{target}': {}", describe(&e)),
                    );
                }
            }
        }

        let Listing { rows, keys, .. } = listing;
        let mut batch = rows.finish_or_empty()?;
        let mut order =
            batch.order_by_index(|i, row| (keys[i], row.get(NAME).as_str().unwrap_or("")));
        if args.has("reverse") {
            order = order.reverse();
        }
        batch.take_rows(order);
        Ok(Value::from_batch(batch))
    }
}

fn list_dir(session: &mut Session, dir: &Path, label: &str, show_all: bool, listing: &mut Listing) {
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
            Ok(meta) => listing.push(&name, Path::new(&file_name), &meta),
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
