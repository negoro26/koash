use std::path::Path;
use std::sync::Arc;

use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{Searcher, SearcherBuilder};
use ignore::WalkBuilder;
use shell_kernel::Session;
use shell_value::{
    CellVal, Column, Datum, RecordBatch, Schema, StructuredDiagnostic, Value, hashline_bytes,
};

use super::common::{
    HASHLINE_COLUMNS, check_killed, hashline_schema, resolve_path, search_file_with_hashline,
};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

static FILE_COLUMNS: [Column; 1] = [Column::new("file")];
static COUNT_COLUMNS: [Column; 2] = [Column::new("file"), Column::new("count")];

pub(super) struct Grep;

static SIGNATURE: Signature = Signature {
    name: "grep",
    aliases: &[],
    description: "Search files or input stream with Hashline anchors",
    flags: &[
        Flag::switch('i', "ignore-case", "case-insensitive match"),
        Flag::switch(
            'n',
            "line-number",
            "accepted; every row already carries its line number",
        ),
        Flag::switch(
            'r',
            "recursive",
            "search directory operands, or the current directory",
        ),
        Flag::short_switch('R', "same as -r"),
        Flag::switch(
            'E',
            "extended-regexp",
            "accepted; patterns are always extended regular expressions",
        ),
        Flag::switch('F', "fixed-strings", "match the pattern as plain text"),
        Flag::switch('w', "word-regexp", "match whole words only"),
        Flag::switch('v', "invert-match", "select lines that do not match"),
        Flag::switch(
            'l',
            "files-with-matches",
            "print only the names of files that match",
        ),
        Flag::switch('c', "count", "print a match count per file"),
        Flag::switch(
            'H',
            "with-filename",
            "accepted; every row already names its file",
        ),
        Flag::switch(
            's',
            "no-messages",
            "do not report unreadable or missing operands",
        ),
        Flag::long_valued("include", "search only files whose name matches the glob"),
        Flag::long_valued("exclude", "skip files whose name matches the glob"),
    ],
    columns: &HASHLINE_COLUMNS,
    input: InputMode::Lines,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Grep {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn reads_input(&self, args: &[String]) -> bool {
        !parse_grep_opts(args).is_ok_and(|o| o.positional.len() > 1 || o.recursive)
    }

    fn run(
        &self,
        session: &mut Session,
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
                        RecordBatch::with_capacity(Schema::of(&FILE_COLUMNS), 16)
                    } else {
                        RecordBatch::with_capacity(hashline_schema(), 64)
                    };

                    if let Some(t_idx) = text_col {
                        let mut seen_files = std::collections::HashSet::new();
                        let mut count: u64 = 0;
                        let mut shared_file: Option<Arc<str>> = None;
                        for (r, row) in in_batch.rows().enumerate() {
                            let Some(text) = row.get(t_idx).as_str() else {
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
                            let file = file_col.map(|f_idx| row.get(f_idx));
                            let file_val = match file {
                                Some(Datum::Str(label)) => {
                                    CellVal::SharedStr(shared_label(&mut shared_file, label))
                                }
                                Some(other) => CellVal::from(other),
                                None => {
                                    CellVal::SharedStr(shared_label(&mut shared_file, "<input>"))
                                }
                            };
                            if opts.files_only {
                                let label = file.and_then(|d| d.as_str()).unwrap_or("<input>");
                                if seen_files.insert(label.to_string()) {
                                    batch.cells.push(file_val);
                                }
                                continue;
                            }
                            let line_val = match line_col {
                                Some(l_idx) => CellVal::from(row.get(l_idx)),
                                None => CellVal::UInt((r + 1) as u64),
                            };
                            let hash_val = match hash_col {
                                Some(h_idx) => CellVal::from(row.get(h_idx)),
                                None => CellVal::Hash(hashline_bytes(text.as_bytes())),
                            };
                            batch.cells.push(file_val);
                            batch.cells.push(line_val);
                            batch.cells.push(hash_val);
                            batch.cells.push(CellVal::Str(text.to_string()));
                        }
                        if opts.count_only {
                            let mut c_batch =
                                RecordBatch::with_capacity(Schema::of(&COUNT_COLUMNS), 1);
                            c_batch.cells.push(CellVal::Str("<input>".to_string()));
                            c_batch.cells.push(CellVal::UInt(count));
                            return Ok(Value::from_batch(c_batch));
                        }
                    } else if let Some(p_idx) = file_col {
                        let mut seen = std::collections::HashSet::new();
                        let mut count_batch =
                            RecordBatch::with_capacity(Schema::of(&COUNT_COLUMNS), 16);
                        for row in in_batch.rows() {
                            if let Some(p_str) = row.get(p_idx).as_str()
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
                        let mut c_batch = RecordBatch::with_capacity(Schema::of(&COUNT_COLUMNS), 1);
                        c_batch.cells.push(CellVal::SharedStr(stdin_cell));
                        c_batch.cells.push(CellVal::UInt(count));
                        return Ok(Value::from_batch(c_batch));
                    }
                    if opts.files_only {
                        let mut f_batch = RecordBatch::with_capacity(Schema::of(&FILE_COLUMNS), 1);
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
            RecordBatch::with_capacity(Schema::of(&FILE_COLUMNS), 32)
        } else {
            RecordBatch::with_capacity(hashline_schema(), 64)
        };
        let mut count_batch = RecordBatch::with_capacity(Schema::of(&COUNT_COLUMNS), 32);

        for path_arg in search_paths {
            let target = resolve_path(session, path_arg);
            let unusable = match std::fs::metadata(&target) {
                Err(e) => Some(describe(&e)),
                Ok(meta) if meta.is_dir() && !opts.recursive => Some("Is a directory".to_string()),
                Ok(_) => None,
            };
            if let Some(reason) = unusable {
                if !opts.no_messages {
                    fail(session, "E_GREP", format!("grep: {path_arg}: {reason}"));
                }
                continue;
            }
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
}

struct GrepOpts {
    case_insensitive: bool,
    fixed_strings: bool,
    word_match: bool,
    invert: bool,
    files_only: bool,
    count_only: bool,
    recursive: bool,
    no_messages: bool,
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
    let args = Args::parse(&SIGNATURE, args)?;
    let patterns = |key: &str| args.values(key).map(str::to_string).collect::<Vec<_>>();
    Ok(GrepOpts {
        case_insensitive: args.has("ignore-case"),
        fixed_strings: args.has("fixed-strings"),
        word_match: args.has("word-regexp"),
        invert: args.has("invert-match"),
        files_only: args.has("files-with-matches"),
        count_only: args.has("count"),
        recursive: args.has("recursive") || args.has("R"),
        no_messages: args.has("no-messages"),
        includes: build_globset(&patterns("include"))?,
        excludes: build_globset(&patterns("exclude"))?,
        positional: args.operands.iter().map(|s| (*s).to_string()).collect(),
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

/// The label's `Arc`, reusing the previous one while consecutive rows carry the same text.
fn shared_label(previous: &mut Option<Arc<str>>, label: &str) -> Arc<str> {
    match previous {
        Some(shared) if **shared == *label => Arc::clone(shared),
        _ => {
            let shared: Arc<str> = Arc::from(label);
            *previous = Some(Arc::clone(&shared));
            shared
        }
    }
}
