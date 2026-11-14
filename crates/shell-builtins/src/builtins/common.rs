use std::path::Path;
use std::path::PathBuf;

use grep_searcher::Searcher;
use grep_searcher::sinks::UTF8;
use shell_kernel::Session;
use shell_value::{Column, Record, Rows, StructuredDiagnostic, hashline_bytes, record};

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

record! {
    pub(crate) struct HashlineRow<'a> {
        file: &'a str,
        line: u64,
        hash: &'a str,
        text: &'a str,
    }
}

pub(crate) const HASHLINE_COLUMNS: &[Column] = <HashlineRow<'static> as Record>::COLUMNS;

pub(crate) type HashlineRows = Rows<HashlineRow<'static>>;

pub(crate) fn push_hashline_row(rows: &mut HashlineRows, file: &str, line: u64, text: &str) {
    let tag = hashline_bytes(text.as_bytes());
    rows.push(&HashlineRow {
        file,
        line,
        hash: std::str::from_utf8(&tag).unwrap_or(""),
        text,
    });
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
    rows: &mut HashlineRows,
    label: &str,
    first: u64,
    lines: impl Iterator<Item = &'a [u8]>,
) {
    for (offset, line) in lines.enumerate() {
        push_hashline_row(
            rows,
            label,
            first + offset as u64,
            &String::from_utf8_lossy(line),
        );
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
    rows: &mut HashlineRows,
) {
    let _ = searcher.search_path(
        matcher,
        path,
        UTF8(|line_number, line| {
            let trimmed = line.strip_suffix('\n').unwrap_or(line);
            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
            push_hashline_row(rows, label, line_number, trimmed);
            Ok(true)
        }),
    );
}

pub(crate) fn kind_of(meta: &std::fs::Metadata) -> &'static str {
    if meta.is_dir() {
        "dir"
    } else if meta.is_symlink() {
        "symlink"
    } else {
        "file"
    }
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
