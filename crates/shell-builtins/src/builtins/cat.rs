use std::sync::Arc;

use shell_kernel::Session;
use shell_value::{RecordBatch, StructuredDiagnostic, Value};

use super::common::{
    HASHLINE_COLUMNS, hashline_schema, operands_read_stdin, push_hashline_rows, resolve_path,
    text_lines,
};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Cat;

static SIGNATURE: Signature = Signature {
    name: "cat",
    aliases: &[],
    description: "Read file contents with Hashline anchors or raw bytes",
    flags: &[
        Flag::switch(
            'r',
            "raw",
            "write the bytes unchanged instead of Hashline rows",
        ),
        Flag::switch(
            'n',
            "number",
            "number the lines of raw output (Hashline rows always carry line numbers)",
        ),
    ],
    columns: &HASHLINE_COLUMNS,
    input: InputMode::Lines,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Cat {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn reads_input(&self, args: &[String]) -> bool {
        operands_read_stdin(&SIGNATURE, args)
    }

    /// Operands are files, or `-` for standard input, which is read once at its position.
    /// Nothing requires UTF-8: raw output is byte-exact and Hashline text is lossy only where
    /// the bytes are not valid. A file that cannot be read is reported and the rest still print.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let raw = args.has("raw");
        let number = args.has("number") && raw;
        if args.operands.is_empty() && !number {
            return Ok(input);
        }
        let operands: &[&str] = if args.operands.is_empty() {
            &["-"]
        } else {
            &args.operands
        };

        let mut stdin = Some(input);
        let mut out = Vec::new();
        let mut batch = RecordBatch::new(hashline_schema());
        let mut numbered = 0u64;
        for operand in operands {
            let (label, bytes) = if *operand == "-" {
                let bytes = stdin.take().map(Value::into_raw_bytes).unwrap_or_default();
                ("<stdin>", bytes)
            } else {
                match std::fs::read(resolve_path(session, operand)) {
                    Ok(bytes) => (*operand, bytes),
                    Err(e) => {
                        fail(
                            session,
                            "E_CAT",
                            format!("cat: {operand}: {}", describe(&e)),
                        );
                        continue;
                    }
                }
            };
            if !raw {
                let label: Arc<str> = Arc::from(label);
                push_hashline_rows(&mut batch, &label, 1, text_lines(&bytes));
            } else if number {
                for line in bytes.split_inclusive(|b| *b == b'\n') {
                    numbered += 1;
                    out.extend_from_slice(format!("{numbered:>6}\t").as_bytes());
                    out.extend_from_slice(line);
                }
            } else {
                out.extend_from_slice(&bytes);
            }
        }
        Ok(if raw {
            Value::from_bytes(out)
        } else {
            Value::from_batch(batch)
        })
    }
}
