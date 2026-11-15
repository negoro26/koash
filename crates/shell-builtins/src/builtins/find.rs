use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::{check_killed, resolve_path};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Find;

static SIGNATURE: Signature = Signature {
    name: "find",
    aliases: &[],
    description: "Walk directory tree respecting ignore rules",
    flags: &["-t", "-type", "-name", "-iname", "-maxdepth", "--hidden"],
    columns: &["path", "ext", "kind", "size"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

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
        let mut root = session.cwd.clone();
        let mut root_label: Option<String> = None;
        let mut include_hidden = false;
        let mut filter_type: Option<String> = None;
        let mut name_matcher: Option<GlobMatcher> = None;
        let mut max_depth: Option<usize> = None;
        let mut iter = args.iter();

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--hidden" => include_hidden = true,
                "-t" | "-type" => {
                    if let Some(t) = iter.next() {
                        filter_type = Some(t.clone());
                    }
                }
                "-name" | "-iname" => {
                    let ci = arg == "-iname";
                    if let Some(pat) = iter.next() {
                        let matcher = GlobBuilder::new(pat)
                            .case_insensitive(ci)
                            .build()
                            .map_err(|e| {
                                StructuredDiagnostic::new("E_GLOB", e.to_string(), None, None)
                            })?
                            .compile_matcher();
                        name_matcher = Some(matcher);
                    }
                }
                "-maxdepth" => {
                    if let Some(val) = iter.next()
                        && let Ok(d) = val.parse::<usize>()
                    {
                        max_depth = Some(d);
                    }
                }
                s if !s.starts_with('-') => {
                    root = resolve_path(session, s);
                    root_label = Some(s.to_string());
                }
                _ => {}
            }
        }

        let mut builder = WalkBuilder::new(&root);
        builder.hidden(!include_hidden);
        if let Some(d) = max_depth {
            builder.max_depth(Some(d));
        }
        let walker = builder.build();
        let mut records = Vec::new();
        for entry_res in walker {
            check_killed(session)?;
            let Ok(entry) = entry_res else {
                continue;
            };
            if entry.path() == root {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let kind = if meta.is_dir() { "dir" } else { "file" };
            if let Some(ref ft) = filter_type {
                let want_dir = ft == "d" || ft == "dir";
                let want_file = ft == "f" || ft == "file";
                if (want_dir && kind != "dir") || (want_file && kind != "file") {
                    continue;
                }
            }
            if let Some(ref matcher) = name_matcher {
                let fname = entry.file_name().to_string_lossy();
                if !matcher.is_match(fname.as_ref()) {
                    continue;
                }
            }
            let rel = entry
                .path()
                .strip_prefix(&root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            let display_path = match &root_label {
                Some(lbl) if lbl != "." => format!("{}/{rel}", lbl.trim_end_matches('/')),
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
            rec.insert("kind".to_string(), JsonVal::String(kind.to_string()));
            rec.insert("size".to_string(), json!(meta.len()));
            records.push(rec);
        }

        Ok(Value::from_records(records))
    }
}
