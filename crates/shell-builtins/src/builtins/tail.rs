use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::{parse_slice_args, read_files_slice};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Tail;

static SIGNATURE: Signature = Signature {
    name: "tail",
    aliases: &[],
    description: "Take last N records, file lines, or raw lines",
    flags: &["-n", "--raw", "-r"],
    columns: &["file", "line", "hash", "text"],
    input: InputMode::Lines,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Tail {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn reads_input(&self, args: &[String]) -> bool {
        parse_slice_args(args, 10).2.is_empty()
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let (n, raw, paths) = parse_slice_args(args, 10);
        if !paths.is_empty() {
            return read_files_slice(session, &paths, n, true, raw);
        }
        match input {
            Value::Batch(mut batch) => {
                batch.tail_rows(n);
                Ok(Value::from_batch(batch))
            }
            Value::Bytes(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let all: Vec<&str> = text.lines().collect();
                let start = all.len().saturating_sub(n);
                let mut out = all[start..].join("\n");
                if !out.is_empty() {
                    out.push('\n');
                }
                Ok(Value::from_bytes(out.into_bytes()))
            }
            other => Ok(other),
        }
    }
}
