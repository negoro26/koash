use std::sync::Arc;

use shell_kernel::Session;
use shell_value::{CellVal, RecordBatch, StructuredDiagnostic, Value, hashline_bytes};

use super::common::{hashline_schema, resolve_path};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Cat;

static SIGNATURE: Signature = Signature {
    name: "cat",
    aliases: &[],
    description: "Read file contents with Hashline anchors or raw bytes",
    flags: &["--raw", "-r", "-n"],
    columns: &["file", "line", "hash", "text"],
    input: InputMode::Lines,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Cat {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn reads_input(&self, args: &[String]) -> bool {
        cat_operands(args).1.is_empty()
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let (raw, paths) = cat_operands(args);
        if paths.is_empty() {
            return Ok(input);
        }
        if raw {
            let mut out = Vec::new();
            for arg in paths {
                let p = resolve_path(session, arg);
                let bytes = std::fs::read(&p)
                    .map_err(|e| StructuredDiagnostic::from_io("E_CAT", &e, None))?;
                out.extend_from_slice(&bytes);
            }
            return Ok(Value::from_bytes(out));
        }
        let mut batch = RecordBatch::new(hashline_schema());
        for arg in paths {
            let p = resolve_path(session, arg);
            let content = std::fs::read_to_string(&p)
                .map_err(|e| StructuredDiagnostic::from_io("E_CAT", &e, None))?;
            let file_cell: Arc<str> = Arc::from(arg.as_str());
            for (idx, line) in content.lines().enumerate() {
                batch.cells.push(CellVal::SharedStr(Arc::clone(&file_cell)));
                batch.cells.push(CellVal::UInt((idx + 1) as u64));
                batch
                    .cells
                    .push(CellVal::Hash(hashline_bytes(line.as_bytes())));
                batch.cells.push(CellVal::Str(line.to_string()));
            }
        }
        Ok(Value::from_batch(batch))
    }
}

fn cat_operands(args: &[String]) -> (bool, Vec<&String>) {
    let mut raw = false;
    let mut paths = Vec::new();
    let mut parse_flags = true;
    for arg in args {
        if parse_flags && arg == "--" {
            parse_flags = false;
        } else if parse_flags && (arg == "--raw" || arg == "-r") {
            raw = true;
        } else if (parse_flags && matches!(arg.as_str(), "-n" | "-b" | "-u" | "-s")) || arg == "-" {
        } else {
            paths.push(arg);
        }
    }
    (raw, paths)
}
