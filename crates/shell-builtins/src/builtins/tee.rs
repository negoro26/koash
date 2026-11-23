use std::fs::OpenOptions;
use std::io::Write;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Tee;

static SIGNATURE: Signature = Signature {
    name: "tee",
    aliases: &[],
    description: "Write piped output to files (undo-tracked) while passing structured data through",
    flags: &[Flag::switch(
        'a',
        "append",
        "append to the given files instead of overwriting",
    )],
    columns: &[],
    input: InputMode::Bytes,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Tee {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&SIGNATURE, args)?;
        let append = parsed.has("append");
        if parsed.operands.is_empty() {
            return Ok(input);
        }

        let bytes = match &input {
            Value::Empty => Vec::new(),
            Value::Bytes(b) => b.clone(),
            Value::Batch(batch) => {
                let mut buf = Vec::new();
                batch.write_lines(&mut buf);
                buf
            }
        };

        for raw in parsed.operands {
            let path = resolve_path(session, raw);
            session.record_output_target(&path);
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(!append)
                .append(append)
                .open(&path);
            match file {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(&bytes) {
                        session.report(StructuredDiagnostic::from_io("E_TEE", &e, None));
                    } else {
                        session.record_mutation(path.to_string_lossy().to_string());
                    }
                }
                Err(e) => {
                    session.report(StructuredDiagnostic::from_io("E_TEE", &e, None));
                }
            }
        }

        Ok(input)
    }
}
