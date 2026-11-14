use ignore::WalkBuilder;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::common::check_killed;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Rename;

static SIGNATURE: Signature = Signature {
    name: "rename",
    aliases: &[],
    description: "Rename symbol across workspace with micro-commit tracking",
    flags: &[],
    columns: &["file", "replacements"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Rename {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        if args.len() < 2 {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "rename requires <old_symbol> <new_symbol>",
                None,
                None,
            ));
        }
        let old_sym = &args[0];
        let new_sym = &args[1];
        let walker = WalkBuilder::new(&session.cwd).build();
        let mut records = Vec::new();

        for entry_res in walker {
            check_killed(session)?;
            let Ok(entry) = entry_res else {
                continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if !content.contains(old_sym.as_str()) {
                continue;
            }
            let count = content.matches(old_sym.as_str()).count();
            session.record_preimage(entry.path());
            let updated = content.replace(old_sym.as_str(), new_sym.as_str());
            std::fs::write(entry.path(), updated)
                .map_err(|e| StructuredDiagnostic::from_io("E_RENAME", &e, None))?;
            let rel = entry
                .path()
                .strip_prefix(&session.cwd)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            session.record_mutation(entry.path().to_string_lossy().to_string());

            let mut rec = StructuredRecord::new();
            rec.insert("file".to_string(), JsonVal::String(rel));
            rec.insert("replacements".to_string(), json!(count));
            records.push(rec);
        }
        Ok(Value::from_records(records))
    }
}
