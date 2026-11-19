use std::ffi::OsStr;
use std::fs::{DirEntry, Metadata};
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
use crate::eza;

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

enum Origin<'a> {
    Operand { typed: &'a str, resolved: &'a Path },
    Child { name: &'a OsStr },
}

trait Visit {
    fn folder(&mut self, typed: &str, resolved: &Path);
    fn entry(&mut self, origin: Origin<'_>, meta: Metadata);
    fn children(&mut self, session: &mut Session, label: &str, dir: &Path, entries: Vec<DirEntry>);
}

struct Listing {
    rows: Rows<LsRow<'static>>,
    keys: Vec<i128>,
    sort: SortBy,
}

impl Visit for Listing {
    fn folder(&mut self, _typed: &str, _resolved: &Path) {}

    fn entry(&mut self, origin: Origin<'_>, meta: Metadata) {
        let (name, path) = match origin {
            Origin::Operand { typed, resolved } => (typed.into(), resolved),
            Origin::Child { name } => (name.to_string_lossy(), Path::new(name)),
        };
        self.keys.push(self.sort.key(&meta));
        self.rows.push(&LsRow {
            ext: path.extension().and_then(|e| e.to_str()).unwrap_or(""),
            kind: kind_of(&meta),
            modified: OrBlank(UnixTime::from_system(meta.modified())),
            name: &name,
            permissions: FileMode(meta.permissions().mode() & 0o7777),
            size: meta.len(),
        });
    }

    fn children(
        &mut self,
        session: &mut Session,
        label: &str,
        _dir: &Path,
        entries: Vec<DirEntry>,
    ) {
        for entry in entries {
            let file_name = entry.file_name();
            match entry.metadata() {
                Ok(meta) => self.entry(Origin::Child { name: &file_name }, meta),
                Err(e) => {
                    fail(
                        session,
                        "E_LS",
                        format!(
                            "cannot access '{label}/{}': {}",
                            file_name.to_string_lossy(),
                            describe(&e)
                        ),
                    );
                }
            }
        }
    }
}

struct Collector {
    files: Vec<eza::Entry>,
    folders: Vec<eza::Folder>,
    with_xattr: bool,
}

impl Visit for Collector {
    fn folder(&mut self, typed: &str, resolved: &Path) {
        self.folders.push(eza::Folder {
            heading: typed.to_string(),
            path: resolved.to_path_buf(),
            entries: Vec::new(),
        });
    }

    fn entry(&mut self, origin: Origin<'_>, meta: Metadata) {
        if let Origin::Operand { typed, resolved } = origin {
            self.files.push(eza::Entry::operand(
                typed,
                resolved.to_path_buf(),
                meta,
                self.with_xattr,
            ));
        }
    }

    fn children(&mut self, session: &mut Session, label: &str, dir: &Path, entries: Vec<DirEntry>) {
        let read = eza::read_children(dir, &entries, self.with_xattr);
        let Some(folder) = self.folders.last_mut() else {
            return;
        };
        folder.entries.reserve(read.len());
        for result in read {
            match result {
                Ok(entry) => folder.entries.push(entry),
                Err((name, e)) => {
                    fail(
                        session,
                        "E_LS",
                        format!(
                            "cannot access '{label}/{}': {}",
                            name.to_string_lossy(),
                            describe(&e)
                        ),
                    );
                }
            }
        }
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

fn sort_of(args: &Args) -> SortBy {
    match args.last_of(&["t", "S"]) {
        Some("t") => SortBy::Time,
        Some("S") => SortBy::Size,
        _ => SortBy::Name,
    }
}

fn walk(session: &mut Session, args: &Args, visit: &mut impl Visit) {
    let show_all = args.has("all") || args.has("almost-all");
    let targets: Vec<&str> = if args.operands.is_empty() {
        vec!["."]
    } else {
        args.operands.clone()
    };
    for target in targets {
        let resolved = resolve_path(session, target);
        if std::fs::metadata(&resolved).is_ok_and(|m| m.is_dir()) {
            list_dir(session, &resolved, target, show_all, visit);
            continue;
        }
        match std::fs::symlink_metadata(&resolved) {
            Ok(meta) => visit.entry(
                Origin::Operand {
                    typed: target,
                    resolved: &resolved,
                },
                meta,
            ),
            Err(e) => {
                fail(
                    session,
                    "E_LS",
                    format!("cannot access '{target}': {}", describe(&e)),
                );
            }
        }
    }
}

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
        let args = Args::parse(&SIGNATURE, args)?;
        let mut listing = Listing {
            rows: Rows::new(),
            keys: Vec::new(),
            sort: sort_of(&args),
        };
        walk(session, &args, &mut listing);

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

    fn terminal_view(
        &self,
        session: &mut Session,
        args: &[String],
    ) -> Option<Result<Vec<u8>, StructuredDiagnostic>> {
        Some(Args::parse(&SIGNATURE, args).map(|args| {
            let lines = args.has("1") && !args.has("l");
            let mut collector = Collector {
                files: Vec::new(),
                folders: Vec::new(),
                with_xattr: !lines,
            };
            walk(session, &args, &mut collector);
            let var = |name: &str| session.env.get(name);
            let environment = eza::Environment {
                no_color: var("NO_COLOR").is_some(),
                ls_colors: var("LS_COLORS"),
                eza_colors: var("EZA_COLORS").or_else(|| var("EXA_COLORS")),
                icon_spacing: var("EZA_ICON_SPACING")
                    .or_else(|| var("EXA_ICON_SPACING"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1),
                columns: var("COLUMNS").and_then(|v| v.parse().ok()),
            };
            let settings = eza::Settings {
                sort: match sort_of(&args) {
                    SortBy::Name => eza::Sort::Name,
                    SortBy::Time => eza::Sort::Newest,
                    SortBy::Size => eza::Sort::Largest,
                },
                reverse: args.has("reverse"),
                lines,
            };
            eza::render(
                eza::Layout {
                    files: collector.files,
                    folders: collector.folders,
                },
                &environment,
                &settings,
            )
        }))
    }
}

fn list_dir(
    session: &mut Session,
    dir: &Path,
    label: &str,
    show_all: bool,
    visit: &mut impl Visit,
) {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) => {
            fail(
                session,
                "E_LS",
                format!("cannot open directory '{label}': {}", describe(&e)),
            );
            return;
        }
    };
    let mut entries = Vec::new();
    for entry in read {
        match entry {
            Ok(entry) => {
                if show_all || !entry.file_name().as_encoded_bytes().starts_with(b".") {
                    entries.push(entry);
                }
            }
            Err(e) => {
                fail(
                    session,
                    "E_LS",
                    format!("cannot read directory '{label}': {}", describe(&e)),
                );
            }
        }
    }
    visit.folder(label, dir);
    visit.children(session, label, dir, entries);
}
