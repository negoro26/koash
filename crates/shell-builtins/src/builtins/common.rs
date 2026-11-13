use std::borrow::Cow;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use grep_searcher::Searcher;
use grep_searcher::sinks::UTF8;
use shell_kernel::Session;
use shell_value::{CellVal, RecordBatch, StructuredDiagnostic, Value, hashline_bytes};

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

pub(crate) fn parse_slice_args(args: &[String], default_n: usize) -> (usize, bool, Vec<&str>) {
    let mut count = default_n;
    let mut raw = false;
    let mut paths = Vec::new();
    let mut parse_flags = true;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if parse_flags && arg == "--" {
            parse_flags = false;
        } else if parse_flags && (arg == "--raw" || arg == "-r") {
            raw = true;
        } else if parse_flags && arg == "-n" {
            if let Some(val) = iter.next()
                && let Ok(n) = val.parse::<usize>()
            {
                count = n;
            }
        } else if parse_flags
            && let Some(rest) = arg.strip_prefix("-n")
            && let Ok(n) = rest.parse::<usize>()
        {
            count = n;
        } else if parse_flags
            && let Some(rest) = arg.strip_prefix('-')
            && !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
            && let Ok(n) = rest.parse::<usize>()
        {
            count = n;
        } else {
            paths.push(arg.as_str());
        }
    }
    (count, raw, paths)
}

pub(crate) fn read_files_slice(
    session: &Session,
    paths: &[&str],
    n: usize,
    from_tail: bool,
    raw: bool,
) -> Result<Value, StructuredDiagnostic> {
    if raw {
        let mut out = String::new();
        for arg in paths {
            let p = resolve_path(session, arg);
            let content = std::fs::read_to_string(&p)
                .map_err(|e| StructuredDiagnostic::from_io("E_READ", &e, None))?;
            let lines: Vec<&str> = content.lines().collect();
            let slice = if from_tail {
                let start = lines.len().saturating_sub(n);
                &lines[start..]
            } else {
                &lines[..n.min(lines.len())]
            };
            for line in slice {
                out.push_str(line);
                out.push('\n');
            }
        }
        return Ok(Value::from_bytes(out.into_bytes()));
    }

    let mut batch = RecordBatch::with_capacity(hashline_schema(), n.min(256));
    for arg in paths {
        let p = resolve_path(session, arg);
        let content = std::fs::read_to_string(&p)
            .map_err(|e| StructuredDiagnostic::from_io("E_READ", &e, None))?;
        let lines: Vec<&str> = content.lines().collect();
        let (start, slice) = if from_tail {
            let s = lines.len().saturating_sub(n);
            (s, &lines[s..])
        } else {
            (0, &lines[..n.min(lines.len())])
        };
        let file_cell: Arc<str> = Arc::from(*arg);
        for (idx, line) in slice.iter().enumerate() {
            batch.cells.push(CellVal::SharedStr(Arc::clone(&file_cell)));
            batch.cells.push(CellVal::UInt((start + idx + 1) as u64));
            batch
                .cells
                .push(CellVal::Hash(hashline_bytes(line.as_bytes())));
            batch.cells.push(CellVal::Str((*line).to_string()));
        }
    }
    Ok(Value::from_batch(batch))
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
