use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Ps;

static SIGNATURE: Signature = Signature {
    name: "ps",
    aliases: &[],
    description: "List running processes",
    flags: &[],
    columns: crate::builtin::columns!["pid", "name", "state", "cmdline"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Ps {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        _args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut records = Vec::new();
        let entries = std::fs::read_dir("/proc")
            .map_err(|e| StructuredDiagnostic::from_io("E_PS", &e, None))?;

        for entry_res in entries {
            let Ok(entry) = entry_res else {
                continue;
            };
            let fname = entry.file_name().to_string_lossy().to_string();
            let Ok(pid) = fname.parse::<u32>() else {
                continue;
            };
            let stat_path = entry.path().join("stat");
            let Ok(stat_str) = std::fs::read_to_string(&stat_path) else {
                continue;
            };
            let (name, state) = parse_proc_stat(&stat_str);
            let cmdline = std::fs::read(entry.path().join("cmdline"))
                .map(|b| {
                    b.split(|&c| c == 0)
                        .filter(|s| !s.is_empty())
                        .map(|s| String::from_utf8_lossy(s).to_string())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();

            let mut rec = StructuredRecord::new();
            rec.insert("pid".to_string(), json!(pid));
            rec.insert("name".to_string(), JsonVal::String(name));
            rec.insert("state".to_string(), JsonVal::String(state));
            rec.insert("cmdline".to_string(), JsonVal::String(cmdline));
            records.push(rec);
        }

        records.sort_by_key(|r| r.get("pid").and_then(|v| v.as_u64()).unwrap_or(0));
        Ok(Value::from_records(records))
    }
}

fn parse_proc_stat(stat: &str) -> (String, String) {
    let Some(l) = stat.find('(') else {
        return (String::new(), String::new());
    };
    let Some(r) = stat.rfind(')') else {
        return (String::new(), String::new());
    };
    let name = stat.get(l + 1..r).unwrap_or("").to_string();
    let state = stat.get(r + 2..r + 3).unwrap_or("").to_string();
    (name, state)
}
