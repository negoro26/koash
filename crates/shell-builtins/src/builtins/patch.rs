use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value, hashline_tag};

use super::common::resolve_path;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Patch;

static SIGNATURE: Signature = Signature {
    name: "patch",
    aliases: &[],
    description: "Apply hash-verified line edit using line:hash anchor",
    flags: &[Flag::long_switch(
        "delete",
        "remove the line; written in place of the new text, which may itself start with a dash",
    )],
    columns: &["file", "line", "old_hash", "new_hash", "status"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Patch {
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
                "patch requires <file> <line:hash> [<new_text> | --delete]",
                None,
                Some("example: patch src/main.rs 42:a9f \"let x = 1;\"".to_string()),
            ));
        }

        let file_arg = &args[0];
        let anchor = &args[1];
        let Some((line_str, expected_hash)) = anchor.split_once(':') else {
            return Err(StructuredDiagnostic::new(
                "E_INVALID_ANCHOR",
                format!("invalid anchor `{anchor}`, expected `line:hash`"),
                None,
                None,
            ));
        };
        let line_num = line_str.parse::<usize>().map_err(|_| {
            StructuredDiagnostic::new(
                "E_INVALID_ANCHOR",
                "line number must be positive integer",
                None,
                None,
            )
        })?;
        if line_num == 0 {
            return Err(StructuredDiagnostic::new(
                "E_INVALID_ANCHOR",
                "line numbers are 1-indexed",
                None,
                None,
            ));
        }

        let is_delete = args.get(2).is_some_and(|a| a == "--delete");
        let new_text = if is_delete {
            String::new()
        } else if let Some(t) = args.get(2) {
            t.clone()
        } else {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "patch requires replacement text or --delete",
                None,
                None,
            ));
        };

        let path = resolve_path(session, file_arg);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| StructuredDiagnostic::from_io("E_PATCH_READ", &e, None))?;
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

        if line_num > lines.len() {
            return Err(StructuredDiagnostic::new(
                "E_HASH_MISMATCH",
                format!(
                    "line {line_num} out of bounds (file has {} lines)",
                    lines.len()
                ),
                None,
                Some("re-read the file with `cat` or `grep` to get fresh anchors".to_string()),
            ));
        }

        let current_line = &lines[line_num - 1];
        let actual_hash = hashline_tag(current_line);
        if actual_hash != expected_hash {
            return Err(StructuredDiagnostic::new(
                "E_HASH_MISMATCH",
                format!(
                    "hash mismatch at {file_arg}:{line_num}: expected `{expected_hash}`, found `{actual_hash}`"
                ),
                None,
                Some("file changed since last read; re-read with `cat` or `grep`".to_string()),
            ));
        }

        let new_hash = if is_delete {
            lines.remove(line_num - 1);
            String::new()
        } else {
            let h = hashline_tag(&new_text);
            lines[line_num - 1] = new_text;
            h
        };

        let mut updated = lines.join("\n");
        if content.ends_with('\n') && !updated.is_empty() {
            updated.push('\n');
        }
        session.record_preimage(&path);
        std::fs::write(&path, updated)
            .map_err(|e| StructuredDiagnostic::from_io("E_PATCH_WRITE", &e, None))?;
        session.record_mutation(path.to_string_lossy().to_string());

        let mut rec = StructuredRecord::new();
        rec.insert("file".to_string(), JsonVal::String(file_arg.clone()));
        rec.insert("line".to_string(), json!(line_num));
        rec.insert("old_hash".to_string(), JsonVal::String(actual_hash));
        rec.insert("new_hash".to_string(), JsonVal::String(new_hash));
        rec.insert(
            "status".to_string(),
            JsonVal::String(if is_delete { "deleted" } else { "patched" }.to_string()),
        );
        Ok(Value::from_records(vec![rec]))
    }
}
