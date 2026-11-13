use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{Searcher, SearcherBuilder};
use ignore::WalkBuilder;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{
    CellVal, RecordBatch, StructuredDiagnostic, StructuredRecord, Value, hashline_bytes,
    hashline_tag,
};

use super::{check_killed, hashline_schema, resolve_path};

pub(crate) fn echo(args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let mut no_newline = false;
    let mut parts: Vec<&str> = Vec::new();
    let mut parse_flags = true;
    for arg in args {
        if parse_flags && arg == "-n" {
            no_newline = true;
        } else {
            parse_flags = false;
            parts.push(arg.as_str());
        }
    }
    let mut out = parts.join(" ").into_bytes();
    if !no_newline {
        out.push(b'\n');
    }
    Ok(Value::from_bytes(out))
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

pub(crate) fn names_files(name: &str, args: &[String]) -> bool {
    match name {
        "cat" => !cat_operands(args).1.is_empty(),
        "head" | "tail" => !parse_slice_args(args, 10).2.is_empty(),
        "grep" => parse_grep_opts(args).is_ok_and(|o| o.positional.len() > 1 || o.recursive),
        _ => false,
    }
}

pub(crate) fn cat(
    session: &Session,
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
            let bytes =
                std::fs::read(&p).map_err(|e| StructuredDiagnostic::from_io("E_CAT", &e, None))?;
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

fn parse_slice_args(args: &[String], default_n: usize) -> (usize, bool, Vec<&str>) {
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

fn read_files_slice(
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

pub(crate) fn head(
    session: &Session,
    args: &[String],
    input: Value,
) -> Result<Value, StructuredDiagnostic> {
    let (n, raw, paths) = parse_slice_args(args, 10);
    if !paths.is_empty() {
        return read_files_slice(session, &paths, n, false, raw);
    }
    match input {
        Value::Batch(mut batch) => {
            batch.truncate_rows(n);
            Ok(Value::from_batch(batch))
        }
        Value::Bytes(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let taken: Vec<&str> = text.lines().take(n).collect();
            let mut out = taken.join("\n");
            if !out.is_empty() {
                out.push('\n');
            }
            Ok(Value::from_bytes(out.into_bytes()))
        }
        other => Ok(other),
    }
}

pub(crate) fn tail(
    session: &Session,
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

struct GrepOpts {
    case_insensitive: bool,
    fixed_strings: bool,
    word_match: bool,
    invert: bool,
    files_only: bool,
    count_only: bool,
    recursive: bool,
    includes: Option<GlobSet>,
    excludes: Option<GlobSet>,
    positional: Vec<String>,
}

fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>, StructuredDiagnostic> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let glob = Glob::new(pat)
            .map_err(|e| StructuredDiagnostic::new("E_GLOB", e.to_string(), None, None))?;
        builder.add(glob);
    }
    let set = builder
        .build()
        .map_err(|e| StructuredDiagnostic::new("E_GLOB", e.to_string(), None, None))?;
    Ok(Some(set))
}

fn parse_grep_opts(args: &[String]) -> Result<GrepOpts, StructuredDiagnostic> {
    let mut case_insensitive = false;
    let mut fixed_strings = false;
    let mut word_match = false;
    let mut invert = false;
    let mut files_only = false;
    let mut count_only = false;
    let mut recursive = false;
    let mut include_pats = Vec::new();
    let mut exclude_pats = Vec::new();
    let mut positional = Vec::new();
    let mut parse_flags = true;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if parse_flags && arg == "--" {
            parse_flags = false;
            continue;
        }
        if parse_flags && let Some(pat) = arg.strip_prefix("--include=") {
            include_pats.push(pat.to_string());
            continue;
        }
        if parse_flags && arg == "--include" {
            if let Some(pat) = iter.next() {
                include_pats.push(pat.clone());
            }
            continue;
        }
        if parse_flags && let Some(pat) = arg.strip_prefix("--exclude=") {
            exclude_pats.push(pat.to_string());
            continue;
        }
        if parse_flags && arg == "--exclude" {
            if let Some(pat) = iter.next() {
                exclude_pats.push(pat.clone());
            }
            continue;
        }
        if parse_flags && arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
            for b in arg.as_bytes()[1..].iter() {
                match b {
                    b'i' => case_insensitive = true,
                    b'F' => fixed_strings = true,
                    b'w' => word_match = true,
                    b'v' => invert = true,
                    b'l' => files_only = true,
                    b'c' => count_only = true,
                    b'r' | b'R' => recursive = true,
                    b'n' | b'E' | b'H' | b'h' | b's' => {}
                    _ => {}
                }
            }
            continue;
        }
        positional.push(arg.clone());
    }

    Ok(GrepOpts {
        case_insensitive,
        fixed_strings,
        word_match,
        invert,
        files_only,
        count_only,
        recursive,
        includes: build_globset(&include_pats)?,
        excludes: build_globset(&exclude_pats)?,
        positional,
    })
}

fn path_matches_filters(opts: &GrepOpts, path: &Path) -> bool {
    let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(ref inc) = opts.includes
        && !inc.is_match(path)
        && !inc.is_match(fname)
    {
        return false;
    }
    if let Some(ref exc) = opts.excludes
        && (exc.is_match(path) || exc.is_match(fname))
    {
        return false;
    }
    true
}

pub(crate) fn grep(
    session: &Session,
    args: &[String],
    input: Value,
) -> Result<Value, StructuredDiagnostic> {
    let opts = parse_grep_opts(args)?;

    let Some(pattern) = opts.positional.first() else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "grep requires a search pattern",
            None,
            Some("usage: grep [-irnFwvlc] [--include <glob>] <pattern> [path...]".to_string()),
        ));
    };

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.case_insensitive)
        .fixed_strings(opts.fixed_strings)
        .word(opts.word_match)
        .build(pattern)
        .map_err(|e| StructuredDiagnostic::new("E_REGEX", e.to_string(), None, None))?;

    let mut searcher = SearcherBuilder::new().invert_match(opts.invert).build();

    if opts.positional.len() == 1 && !(opts.recursive && matches!(input, Value::Empty)) {
        match input {
            Value::Batch(in_batch) => {
                let text_col = in_batch.col_index("text");
                let file_col = in_batch
                    .col_index("file")
                    .or_else(|| in_batch.col_index("path"));
                let line_col = in_batch.col_index("line");
                let hash_col = in_batch.col_index("hash");

                let mut batch = if opts.files_only {
                    let cols: Arc<[Cow<'static, str>]> = Arc::new([Cow::Borrowed("file")]);
                    RecordBatch::with_capacity(cols, 16)
                } else {
                    RecordBatch::with_capacity(hashline_schema(), 64)
                };

                if let Some(t_idx) = text_col {
                    let mut seen_files = std::collections::HashSet::new();
                    let mut count: u64 = 0;
                    for r in 0..in_batch.nrows() {
                        let row = in_batch.row(r);
                        let Some(text) = row[t_idx].as_str() else {
                            continue;
                        };
                        let mut matched = false;
                        let _ = searcher.search_slice(
                            &matcher,
                            text.as_bytes(),
                            UTF8(|_, _| {
                                matched = true;
                                Ok(false)
                            }),
                        );
                        if !matched {
                            continue;
                        }
                        if opts.count_only {
                            count += 1;
                            continue;
                        }
                        let file_val = match file_col {
                            Some(f_idx) => row[f_idx].clone(),
                            None => CellVal::Str("<input>".to_string()),
                        };
                        if opts.files_only {
                            let label = file_val.as_str().unwrap_or("<input>").to_string();
                            if seen_files.insert(label) {
                                batch.cells.push(file_val);
                            }
                            continue;
                        }
                        let line_val = match line_col {
                            Some(l_idx) => row[l_idx].clone(),
                            None => CellVal::UInt((r + 1) as u64),
                        };
                        let hash_val = match hash_col {
                            Some(h_idx) => row[h_idx].clone(),
                            None => CellVal::Hash(hashline_bytes(text.as_bytes())),
                        };
                        batch.cells.push(file_val);
                        batch.cells.push(line_val);
                        batch.cells.push(hash_val);
                        batch.cells.push(CellVal::Str(text.to_string()));
                    }
                    if opts.count_only {
                        let cols: Arc<[Cow<'static, str>]> =
                            Arc::new([Cow::Borrowed("file"), Cow::Borrowed("count")]);
                        let mut c_batch = RecordBatch::with_capacity(cols, 1);
                        c_batch.cells.push(CellVal::Str("<input>".to_string()));
                        c_batch.cells.push(CellVal::UInt(count));
                        return Ok(Value::from_batch(c_batch));
                    }
                } else if let Some(p_idx) = file_col {
                    let mut seen = std::collections::HashSet::new();
                    let mut count_batch = RecordBatch::with_capacity(
                        Arc::new([Cow::Borrowed("file"), Cow::Borrowed("count")]),
                        16,
                    );
                    for r in 0..in_batch.nrows() {
                        let row = in_batch.row(r);
                        if let Some(p_str) = row[p_idx].as_str()
                            && seen.insert(p_str.to_string())
                        {
                            let target = resolve_path(session, p_str);
                            if target.is_file() && path_matches_filters(&opts, &target) {
                                search_target_file(
                                    &mut searcher,
                                    &matcher,
                                    &target,
                                    p_str,
                                    &opts,
                                    &mut batch.cells,
                                    &mut count_batch.cells,
                                );
                            }
                        }
                    }
                    if opts.count_only {
                        return Ok(Value::from_batch(count_batch));
                    }
                }
                return Ok(Value::from_batch(batch));
            }
            _ => {
                let bytes = input.into_raw_bytes();
                let stdin_cell: Arc<str> = Arc::from("<stdin>");
                if opts.count_only {
                    let mut count: u64 = 0;
                    let _ = searcher.search_slice(
                        &matcher,
                        &bytes,
                        UTF8(|_, _| {
                            count += 1;
                            Ok(true)
                        }),
                    );
                    let cols: Arc<[Cow<'static, str>]> =
                        Arc::new([Cow::Borrowed("file"), Cow::Borrowed("count")]);
                    let mut c_batch = RecordBatch::with_capacity(cols, 1);
                    c_batch.cells.push(CellVal::SharedStr(stdin_cell));
                    c_batch.cells.push(CellVal::UInt(count));
                    return Ok(Value::from_batch(c_batch));
                }
                if opts.files_only {
                    let cols: Arc<[Cow<'static, str>]> = Arc::new([Cow::Borrowed("file")]);
                    let mut f_batch = RecordBatch::with_capacity(cols, 1);
                    let mut hit = false;
                    let _ = searcher.search_slice(
                        &matcher,
                        &bytes,
                        UTF8(|_, _| {
                            hit = true;
                            Ok(false)
                        }),
                    );
                    if hit {
                        f_batch.cells.push(CellVal::SharedStr(stdin_cell));
                    }
                    return Ok(Value::from_batch(f_batch));
                }
                let mut batch = RecordBatch::with_capacity(hashline_schema(), 64);
                let _ = searcher.search_slice(
                    &matcher,
                    &bytes,
                    UTF8(|line_number, line| {
                        let trimmed = line.strip_suffix('\n').unwrap_or(line);
                        let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
                        batch
                            .cells
                            .push(CellVal::SharedStr(Arc::clone(&stdin_cell)));
                        batch.cells.push(CellVal::UInt(line_number));
                        batch
                            .cells
                            .push(CellVal::Hash(hashline_bytes(trimmed.as_bytes())));
                        batch.cells.push(CellVal::Str(trimmed.to_string()));
                        Ok(true)
                    }),
                );
                return Ok(Value::from_batch(batch));
            }
        }
    }

    let search_paths: Vec<&str> = if opts.positional.len() > 1 {
        opts.positional[1..].iter().map(|s| s.as_str()).collect()
    } else {
        vec!["."]
    };

    let mut batch = if opts.files_only {
        RecordBatch::with_capacity(Arc::new([Cow::Borrowed("file")]), 32)
    } else {
        RecordBatch::with_capacity(hashline_schema(), 64)
    };
    let mut count_batch = RecordBatch::with_capacity(
        Arc::new([Cow::Borrowed("file"), Cow::Borrowed("count")]),
        32,
    );

    for path_arg in search_paths {
        let target = resolve_path(session, path_arg);
        let walker = WalkBuilder::new(&target).build();
        for entry_res in walker {
            check_killed(session)?;
            let Ok(entry) = entry_res else {
                continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            if !path_matches_filters(&opts, entry.path()) {
                continue;
            }
            let p_str = entry.path().to_string_lossy();
            search_target_file(
                &mut searcher,
                &matcher,
                entry.path(),
                &p_str,
                &opts,
                &mut batch.cells,
                &mut count_batch.cells,
            );
        }
    }

    if opts.count_only {
        Ok(Value::from_batch(count_batch))
    } else {
        Ok(Value::from_batch(batch))
    }
}

fn search_target_file(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    path: &Path,
    label: &str,
    opts: &GrepOpts,
    cells: &mut Vec<CellVal>,
    count_cells: &mut Vec<CellVal>,
) {
    if opts.count_only {
        let mut count: u64 = 0;
        let _ = searcher.search_path(
            matcher,
            path,
            UTF8(|_, _| {
                count += 1;
                Ok(true)
            }),
        );
        if count > 0 {
            count_cells.push(CellVal::Str(label.to_string()));
            count_cells.push(CellVal::UInt(count));
        }
        return;
    }
    if opts.files_only {
        let mut hit = false;
        let _ = searcher.search_path(
            matcher,
            path,
            UTF8(|_, _| {
                hit = true;
                Ok(false)
            }),
        );
        if hit {
            cells.push(CellVal::Str(label.to_string()));
        }
        return;
    }
    search_file_with_hashline(searcher, matcher, path, label, cells);
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

pub(crate) fn patch(session: &mut Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    if args.len() < 2 {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "patch requires <file> <line:hash> [<new_text> | --delete]",
            None,
            Some("example: patch src/main.rs 42:a9f \"let x = 1;\"".to_string()),
        ));
    }

    let file_arg = &args[0];
    let anchor = &args[1];
    let Some((line_str, expected_hash)) = anchor.split_once(':') else {
        return Err(StructuredDiagnostic::new(
            "E_INVALID_ANCHOR",
            format!("invalid anchor `{anchor}`, expected `line:hash`"),
            None,
            None,
        ));
    };
    let line_num = line_str.parse::<usize>().map_err(|_| {
        StructuredDiagnostic::new(
            "E_INVALID_ANCHOR",
            "line number must be positive integer",
            None,
            None,
        )
    })?;
    if line_num == 0 {
        return Err(StructuredDiagnostic::new(
            "E_INVALID_ANCHOR",
            "line numbers are 1-indexed",
            None,
            None,
        ));
    }

    let is_delete = args.get(2).is_some_and(|a| a == "--delete");
    let new_text = if is_delete {
        String::new()
    } else if let Some(t) = args.get(2) {
        t.clone()
    } else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "patch requires replacement text or --delete",
            None,
            None,
        ));
    };

    let path = resolve_path(session, file_arg);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| StructuredDiagnostic::from_io("E_PATCH_READ", &e, None))?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    if line_num > lines.len() {
        return Err(StructuredDiagnostic::new(
            "E_HASH_MISMATCH",
            format!(
                "line {line_num} out of bounds (file has {} lines)",
                lines.len()
            ),
            None,
            Some("re-read the file with `cat` or `grep` to get fresh anchors".to_string()),
        ));
    }

    let current_line = &lines[line_num - 1];
    let actual_hash = hashline_tag(current_line);
    if actual_hash != expected_hash {
        return Err(StructuredDiagnostic::new(
            "E_HASH_MISMATCH",
            format!(
                "hash mismatch at {file_arg}:{line_num}: expected `{expected_hash}`, found `{actual_hash}`"
            ),
            None,
            Some("file changed since last read; re-read with `cat` or `grep`".to_string()),
        ));
    }

    let new_hash = if is_delete {
        lines.remove(line_num - 1);
        String::new()
    } else {
        let h = hashline_tag(&new_text);
        lines[line_num - 1] = new_text;
        h
    };

    let mut updated = lines.join("\n");
    if content.ends_with('\n') && !updated.is_empty() {
        updated.push('\n');
    }
    session.record_preimage(&path);
    std::fs::write(&path, updated)
        .map_err(|e| StructuredDiagnostic::from_io("E_PATCH_WRITE", &e, None))?;
    session.record_mutation(path.to_string_lossy().to_string());

    let mut rec = StructuredRecord::new();
    rec.insert("file".to_string(), JsonVal::String(file_arg.clone()));
    rec.insert("line".to_string(), json!(line_num));
    rec.insert("old_hash".to_string(), JsonVal::String(actual_hash));
    rec.insert("new_hash".to_string(), JsonVal::String(new_hash));
    rec.insert(
        "status".to_string(),
        JsonVal::String(if is_delete { "deleted" } else { "patched" }.to_string()),
    );
    Ok(Value::from_records(vec![rec]))
}
