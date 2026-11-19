use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::{Searcher, SearcherBuilder};
use ignore::WalkBuilder;
use rustc_hash::FxHashSet;
use shell_kernel::Session;
use shell_value::{
    Column, Datum, DynRows, RecordBatch, Rows, Schema, StructuredDiagnostic, Value, record,
};

use super::common::{
    HASHLINE_COLUMNS, HashlineRows, check_killed, push_hashline_row, resolve_path,
    search_file_with_hashline,
};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

record! { struct FileRow<'a> { file: &'a str } }
record! { struct CountRow<'a> { file: &'a str, count: u64 } }

enum Found {
    Lines(HashlineRows),
    Files(Rows<FileRow<'static>>),
    Counts(Rows<CountRow<'static>>),
}

impl Found {
    fn new(opts: &GrepOpts) -> Self {
        if opts.count_only {
            Found::Counts(Rows::new())
        } else if opts.files_only {
            Found::Files(Rows::new())
        } else {
            Found::Lines(HashlineRows::with_capacity(64))
        }
    }

    fn finish(self) -> Result<RecordBatch, StructuredDiagnostic> {
        match self {
            Found::Lines(rows) => rows.finish(),
            Found::Files(rows) => rows.finish(),
            Found::Counts(rows) => rows.finish(),
        }
    }
}

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
            "number input rows that carry no line column",
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
    columns: HASHLINE_COLUMNS,
    input: InputMode::Lines,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
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
                Value::Batch(batch) => return grep_table(&opts, &matcher, batch),
                _ => {
                    let bytes = input.into_raw_bytes();
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
                        let mut counts = Rows::<CountRow<'static>>::new();
                        counts.push(&CountRow {
                            file: "<stdin>",
                            count,
                        });
                        return Ok(Value::from_batch(counts.finish()?));
                    }
                    if opts.files_only {
                        let mut files = Rows::<FileRow<'static>>::new();
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
                            files.push(&FileRow { file: "<stdin>" });
                        }
                        return Ok(Value::from_batch(files.finish()?));
                    }
                    let mut rows = HashlineRows::with_capacity(64);
                    let _ = searcher.search_slice(
                        &matcher,
                        &bytes,
                        UTF8(|line_number, line| {
                            let trimmed = line.strip_suffix('\n').unwrap_or(line);
                            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
                            push_hashline_row(&mut rows, "<stdin>", line_number, trimmed);
                            Ok(true)
                        }),
                    );
                    return Ok(Value::from_batch(rows.finish()?));
                }
            }
        }

        let search_paths: Vec<&str> = if opts.positional.len() > 1 {
            opts.positional[1..].iter().map(|s| s.as_str()).collect()
        } else {
            vec!["."]
        };

        let mut found = Found::new(&opts);
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
                search_target_file(&mut searcher, &matcher, entry.path(), &p_str, &mut found);
            }
        }

        Ok(Value::from_batch(found.finish()?))
    }
}

struct GrepOpts {
    case_insensitive: bool,
    fixed_strings: bool,
    word_match: bool,
    invert: bool,
    files_only: bool,
    count_only: bool,
    line_numbers: bool,
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
        line_numbers: args.has("line-number"),
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
    found: &mut Found,
) {
    match found {
        Found::Counts(rows) => {
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
                rows.push(&CountRow { file: label, count });
            }
        }
        Found::Files(rows) => {
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
                rows.push(&FileRow { file: label });
            }
        }
        Found::Lines(rows) => search_file_with_hashline(searcher, matcher, path, label, rows),
    }
}

fn grep_table(
    opts: &GrepOpts,
    matcher: &RegexMatcher,
    mut batch: RecordBatch,
) -> Result<Value, StructuredDiagnostic> {
    let cols = batch
        .schema()
        .text_column()
        .or_else(|| batch.col_index("text"))
        .map_or(0..batch.ncols(), |col| col..col + 1);
    let mut line = Vec::new();
    let mut scratch = String::new();
    let mut seen = 0u64;
    let mut numbers = Vec::new();
    batch.retain_rows(|row| {
        seen += 1;
        let bytes = match (cols.len(), row.get(cols.start)) {
            (1, Datum::Str(text)) => text.as_bytes(),
            _ => {
                line.clear();
                row.write_line(cols.clone(), &mut line, &mut scratch);
                &line
            }
        };
        let kept = matcher.is_match(bytes).is_ok_and(|hit| hit) != opts.invert;
        if kept && opts.line_numbers {
            numbers.push(seen);
        }
        kept
    });

    if opts.count_only {
        let mut counts = Rows::<CountRow<'static>>::new();
        counts.push(&CountRow {
            file: "<input>",
            count: batch.nrows() as u64,
        });
        return Ok(Value::from_batch(counts.finish()?));
    }
    if opts.files_only {
        let file_col = batch.col_index("file");
        let mut files = Rows::<FileRow<'static>>::new();
        let mut named = FxHashSet::default();
        for row in batch.rows() {
            let file = file_col.and_then(|col| row.get(col).as_str());
            if named.insert(file) {
                files.push(&FileRow {
                    file: file.unwrap_or("<input>"),
                });
            }
        }
        return Ok(Value::from_batch(files.finish()?));
    }
    if opts.line_numbers && batch.col_index("line").is_none() {
        let mut columns = vec![Column::new("line")];
        columns.extend(batch.schema().columns().iter().cloned());
        let mut rows = DynRows::new(Schema::from(columns));
        for (row, number) in batch.rows().zip(numbers) {
            rows.push_with(|out| {
                out.put(Datum::UInt(number));
                for cell in row.iter() {
                    out.put(cell);
                }
            });
        }
        return Ok(Value::from_batch(rows.finish()?));
    }
    Ok(Value::from_batch(batch))
}
