use std::borrow::Cow;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use grep_searcher::Searcher;
use grep_searcher::sinks::UTF8;
use shell_kernel::Session;
use shell_value::{CellVal, RecordBatch, StructuredDiagnostic, hashline_bytes};

use crate::args::Args;
use crate::builtin::Signature;

pub(crate) fn check_killed(session: &Session) -> Result<(), StructuredDiagnostic> {
    if session.kill.is_killed() {
        return Err(StructuredDiagnostic::new(
            "E_KILLED",
            "command killed",
            None,
            None,
        ));
    }
    Ok(())
}

pub(crate) fn resolve_path(session: &Session, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        session.cwd.join(p)
    }
}

pub(crate) fn hashline_schema() -> Arc<[Cow<'static, str>]> {
    Arc::new([
        Cow::Borrowed("file"),
        Cow::Borrowed("line"),
        Cow::Borrowed("hash"),
        Cow::Borrowed("text"),
    ])
}

/// Splits bytes into lines like `str::lines` does, without requiring UTF-8: a final newline
/// ends the last line instead of starting another, and a `\r` before the `\n` is dropped.
pub(crate) fn text_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    (!bytes.is_empty())
        .then(|| body.split(|b| *b == b'\n'))
        .into_iter()
        .flatten()
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

/// Appends one Hashline row per line, numbered from `first`. Invalid UTF-8 shows as U+FFFD.
pub(crate) fn push_hashline_rows<'a>(
    batch: &mut RecordBatch,
    label: &Arc<str>,
    first: u64,
    lines: impl Iterator<Item = &'a [u8]>,
) {
    for (offset, line) in lines.enumerate() {
        let text = String::from_utf8_lossy(line).into_owned();
        batch.cells.push(CellVal::SharedStr(Arc::clone(label)));
        batch.cells.push(CellVal::UInt(first + offset as u64));
        batch
            .cells
            .push(CellVal::Hash(hashline_bytes(text.as_bytes())));
        batch.cells.push(CellVal::Str(text));
    }
}

pub(crate) fn usage_error(message: impl Into<String>, usage: &str) -> StructuredDiagnostic {
    StructuredDiagnostic::new("E_ARGS", message, None, Some(format!("usage: {usage}")))
}

/// Whether a command that reads files falls back to standard input: it has no operands or a
/// lone `-`. An argument error means nothing is read, and `run` reports it.
pub(crate) fn operands_read_stdin(sig: &'static Signature, args: &[String]) -> bool {
    Args::parse(sig, args).is_ok_and(|a| a.operands.is_empty() || a.operands.contains(&"-"))
}

pub(crate) fn search_file_with_hashline(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    path: &Path,
    label: &str,
    cells: &mut Vec<CellVal>,
) {
    let mut shared_label: Option<Arc<str>> = None;
    let _ = searcher.search_path(
        matcher,
        path,
        UTF8(|line_number, line| {
            let trimmed = line.strip_suffix('\n').unwrap_or(line);
            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
            let file_cell = shared_label.get_or_insert_with(|| Arc::from(label));
            cells.push(CellVal::SharedStr(Arc::clone(file_cell)));
            cells.push(CellVal::UInt(line_number));
            cells.push(CellVal::Hash(hashline_bytes(trimmed.as_bytes())));
            cells.push(CellVal::Str(trimmed.to_string()));
            Ok(true)
        }),
    );
}

pub(crate) fn format_unix_iso(secs: u64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

// Howard Hinnant's civil_from_days: https://howardhinnant.github.io/date_algorithms.html#civil_from_days
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}

pub(crate) fn job_index(
    session: &Session,
    arg: Option<&String>,
    default: Option<usize>,
) -> Option<usize> {
    match arg.and_then(|s| s.trim_start_matches('%').parse::<usize>().ok()) {
        Some(id) => session.jobs.iter().position(|j| j.id == id),
        None => default,
    }
}

pub(crate) fn sole_stage_only(sig: &Signature) -> StructuredDiagnostic {
    StructuredDiagnostic::new(
        "E_PLACEMENT",
        format!("`{}` runs only as a whole foreground pipeline", sig.name),
        None,
        None,
    )
}
