use std::io::{BufRead, BufReader};
use std::path::Path;

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
            let rel = entry
                .path()
                .strip_prefix(&session.cwd)
                .unwrap_or(entry.path())
                .to_string_lossy();
            search_file(entry.path(), &rel, symbol, &mut rows);
        }
        Ok(Value::from_batch(rows.finish()?))
    }
}

fn search_file(path: &Path, label: &str, symbol: &str, rows: &mut HashlineRows) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut reader = BufReader::new(file);
    if reader.fill_buf().map_or(true, |head| head.contains(&0)) {
        return;
    }
    let mut buf = Vec::new();
    let mut line_no = 0u64;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => line_no += 1,
        }
        if buf.contains(&0) {
            return;
        }
        let Ok(line) = std::str::from_utf8(&buf) else {
            continue;
        };
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if contains_word(line, symbol) {
            push_hashline_row(rows, label, line_no, line);
        }
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
