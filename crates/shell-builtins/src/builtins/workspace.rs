use grep_regex::RegexMatcherBuilder;
use grep_searcher::Searcher;
use ignore::WalkBuilder;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{RecordBatch, StructuredDiagnostic, StructuredRecord, Value};

use super::text::search_file_with_hashline;
use super::{check_killed, hashline_schema};

pub(crate) fn diag(session: &Session) -> Result<Value, StructuredDiagnostic> {
    if !session.cwd.join("Cargo.toml").is_file() {
        return Ok(Value::from_records(Vec::new()));
    }
    let output = std::process::Command::new("cargo")
        .args(["check", "--message-format=json", "--quiet"])
        .current_dir(&session.cwd)
        .output()
        .map_err(|e| StructuredDiagnostic::from_io("E_DIAG", &e, None))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut records = Vec::new();
    for line in stdout.lines() {
        let Ok(val) = serde_json::from_str::<JsonVal>(line) else {
            continue;
        };
        if val.get("reason").and_then(|v| v.as_str()) != Some("compiler-message") {
            continue;
        }
        let Some(msg) = val.get("message") else {
            continue;
        };
        let level = msg.get("level").and_then(|v| v.as_str()).unwrap_or("info");
        if level != "error" && level != "warning" {
            continue;
        }
        let text = msg.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let code = msg
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let span = msg.get("spans").and_then(|s| s.get(0));
        let file = span
            .and_then(|s| s.get("file_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let line_num = span
            .and_then(|s| s.get("line_start"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let mut rec = StructuredRecord::new();
        rec.insert("file".to_string(), JsonVal::String(file.to_string()));
        rec.insert("line".to_string(), json!(line_num));
        rec.insert("severity".to_string(), JsonVal::String(level.to_string()));
        rec.insert("code".to_string(), JsonVal::String(code.to_string()));
        rec.insert("message".to_string(), JsonVal::String(text.to_string()));
        records.push(rec);
    }
    Ok(Value::from_records(records))
}

pub(crate) fn refs(session: &Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let Some(symbol) = args.first() else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "refs requires a symbol name",
            None,
            None,
        ));
    };
    let matcher = RegexMatcherBuilder::new()
        .word(true)
        .build(symbol)
        .map_err(|e| StructuredDiagnostic::new("E_REGEX", e.to_string(), None, None))?;
    let mut searcher = Searcher::new();
    let mut batch = RecordBatch::with_capacity(hashline_schema(), 32);
    let walker = WalkBuilder::new(&session.cwd).build();
    for entry_res in walker {
        check_killed(session)?;
        let Ok(entry) = entry_res else {
            continue;
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&session.cwd)
            .unwrap_or(entry.path())
            .to_string_lossy();
        search_file_with_hashline(
            &mut searcher,
            &matcher,
            entry.path(),
            &rel,
            &mut batch.cells,
        );
    }
    Ok(Value::from_batch(batch))
}

pub(crate) fn rename(
    session: &mut Session,
    args: &[String],
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
