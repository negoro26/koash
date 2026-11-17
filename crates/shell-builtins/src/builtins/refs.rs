use ignore::WalkBuilder;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::{HASHLINE_COLUMNS, HashlineRows, check_killed, push_hashline_row};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Refs;

static SIGNATURE: Signature = Signature {
    name: "refs",
    aliases: &[],
    description: "Find symbol references across workspace",
    flags: &[],
    columns: HASHLINE_COLUMNS,
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
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
        let Some(symbol) = args.first().filter(|s| !s.is_empty()) else {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "refs requires a symbol name",
                None,
                None,
            ));
        };
        let mut rows = HashlineRows::with_capacity(32);
        let walker = WalkBuilder::new(&session.cwd).build();
        for entry_res in walker {
            check_killed(session)?;
            let Ok(entry) = entry_res else {
                continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let rel = entry
                .path()
                .strip_prefix(&session.cwd)
                .unwrap_or(entry.path())
                .to_string_lossy();
            for (idx, line) in text.lines().enumerate() {
                if contains_word(line, symbol) {
                    push_hashline_row(&mut rows, &rel, idx as u64 + 1, line);
                }
            }
        }
        Ok(Value::from_batch(rows.finish()?))
    }
}

fn contains_word(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
