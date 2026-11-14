use grep_regex::RegexMatcherBuilder;
use grep_searcher::Searcher;
use ignore::WalkBuilder;
use shell_kernel::Session;
use shell_value::{RecordBatch, StructuredDiagnostic, Value};

use super::common::{HASHLINE_COLUMNS, check_killed, hashline_schema, search_file_with_hashline};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Refs;

static SIGNATURE: Signature = Signature {
    name: "refs",
    aliases: &[],
    description: "Find symbol references across workspace",
    flags: &[],
    columns: &HASHLINE_COLUMNS,
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Refs {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
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
}
