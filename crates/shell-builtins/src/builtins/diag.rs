use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Diag;

static SIGNATURE: Signature = Signature {
    name: "diag",
    aliases: &[],
    description: "Run project diagnostics (cargo check) and emit structured errors",
    flags: &[],
    columns: &["file", "line", "severity", "code", "message"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Diag {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        _args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
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
}
