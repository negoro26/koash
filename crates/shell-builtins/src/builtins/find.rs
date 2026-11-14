use globset::GlobBuilder;
use ignore::WalkBuilder;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::{check_killed, resolve_path, usage_error};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Find;

static SIGNATURE: Signature = Signature {
    name: "find",
    aliases: &[],
    description: "Walk directory tree respecting ignore rules",
    flags: &[
        Flag::predicate("type", "d or dir, f or file").with_short('t'),
        Flag::predicate("name", "glob matched against the file name"),
        Flag::predicate("iname", "like -name, ignoring case"),
        Flag::predicate("maxdepth", "descend at most N levels"),
        Flag::long_switch("hidden", "include entries whose name starts with a dot"),
    ],
    columns: crate::builtin::columns!["path", "ext", "kind", "size"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
}

impl Builtin for Find {
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
        let usage = "find [DIR] [-type d|f] [-name GLOB] [-iname GLOB] [-maxdepth N] [--hidden]";
        let root_label = match args.operands.as_slice() {
            [] => None,
            [one] => Some(*one),
            _ => {
                return Err(usage_error(
                    "find: only one starting directory is supported",
                    usage,
                ));
            }
        };
        let root = root_label.map_or_else(|| session.cwd.clone(), |l| resolve_path(session, l));
        let want_kind = match args.value("type") {
            None => None,
            Some("d" | "dir") => Some(Kind::Dir),
            Some("f" | "file") => Some(Kind::File),
            Some(other) => {
                return Err(usage_error(format!("find: unknown type '{other}'"), usage));
            }
        };
        let max_depth = args.number::<usize>(&SIGNATURE, "maxdepth")?;
        let name_matcher = match (args.value("iname"), args.value("name")) {
            (None, None) => None,
            (iname, name) => {
                let (pattern, case_insensitive) = match iname {
                    Some(p) => (p, true),
                    None => (name.unwrap_or_default(), false),
                };
                let glob = GlobBuilder::new(pattern)
                    .case_insensitive(case_insensitive)
                    .build()
                    .map_err(|e| StructuredDiagnostic::new("E_GLOB", e.to_string(), None, None))?;
                Some(glob.compile_matcher())
            }
        };

        if let Err(e) = std::fs::metadata(&root) {
            let label = root_label.unwrap_or(".");
            fail(
                session,
                "E_FIND",
                format!("find: '{label}': {}", describe(&e)),
            );
            return Ok(Value::from_records(Vec::new()));
        }

        let mut builder = WalkBuilder::new(&root);
        builder.hidden(!args.has("hidden"));
        if let Some(d) = max_depth {
            builder.max_depth(Some(d));
        }
        let mut records = Vec::new();
        for entry_res in builder.build() {
            check_killed(session)?;
            let Ok(entry) = entry_res else {
                continue;
            };
            if entry.path() == root {
                continue;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            let kind = if is_dir { Kind::Dir } else { Kind::File };
            if want_kind.is_some_and(|want| want != kind) {
                continue;
            }
            if let Some(matcher) = &name_matcher
                && !matcher.is_match(entry.file_name().to_string_lossy().as_ref())
            {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let rel = entry
                .path()
                .strip_prefix(&root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            let display_path = match root_label {
                Some(label) if label != "." => format!("{}/{rel}", label.trim_end_matches('/')),
                _ => rel,
            };
            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();

            let mut rec = StructuredRecord::new();
            rec.insert("path".to_string(), JsonVal::String(display_path));
            rec.insert("ext".to_string(), JsonVal::String(ext));
            rec.insert(
                "kind".to_string(),
                JsonVal::String(if is_dir { "dir" } else { "file" }.to_string()),
            );
            rec.insert("size".to_string(), json!(meta.len()));
            records.push(rec);
        }

        Ok(Value::from_records(records))
    }
}
