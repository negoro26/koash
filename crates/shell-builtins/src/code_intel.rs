use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::Path;

use ignore::WalkBuilder;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value, hashline_bytes, hashline_tag};
use tree_sitter::{Node, Parser};

thread_local! {
    static RUST_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

fn with_rust_parser<F, R>(f: F) -> R
where
    F: FnOnce(&mut Parser) -> R,
{
    RUST_PARSER.with(|cell| {
        let mut opt = cell.borrow_mut();
        let parser = opt.get_or_insert_with(|| {
            let mut p = Parser::new();
            let _ = p.set_language(&tree_sitter_rust::LANGUAGE.into());
            p
        });
        f(parser)
    })
}

pub fn run_prewalk(session: &Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let mut root = session.cwd.clone();
    let mut budget_tokens: usize = 2000;
    let mut deep = false;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if arg == "--budget"
            && let Some(val) = iter.next()
            && let Ok(n) = val.parse::<usize>()
        {
            budget_tokens = n;
        } else if arg == "--deep" {
            deep = true;
        } else if !arg.starts_with('-') {
            root = resolve_path(session, arg);
        }
    }

    let walker = WalkBuilder::new(&root).build();
    let mut candidates: Vec<(usize, String, String)> = Vec::new();

    for entry_res in walker {
        let Ok(entry) = entry_res else {
            continue;
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };

        let is_rs = path.extension().and_then(|e| e.to_str()) == Some("rs");
        let skeleton = if deep && is_rs {
            extract_rust_treesitter(&bytes)
        } else {
            extract_skeleton_bytes(&bytes)
        };

        if skeleton.is_empty() && !rel.ends_with("Cargo.toml") && !rel.ends_with("package.json") {
            continue;
        }
        let rank = score_centrality(&rel, &skeleton);
        let summary = if skeleton.is_empty() {
            let text = String::from_utf8_lossy(&bytes);
            text.lines().take(12).collect::<Vec<_>>().join("\n")
        } else {
            skeleton
        };
        candidates.push((rank, rel, summary));
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut used_tokens = 0usize;
    let mut records = Vec::new();
    for (rank, file, skeleton) in candidates {
        let est_tokens = (skeleton.len() + file.len() + 16) / 4;
        if used_tokens + est_tokens > budget_tokens && !records.is_empty() {
            break;
        }
        used_tokens += est_tokens;
        let mut rec = StructuredRecord::new();
        rec.insert("file".to_string(), JsonVal::String(file));
        rec.insert("rank".to_string(), json!(rank));
        rec.insert("tokens".to_string(), json!(est_tokens));
        rec.insert("skeleton".to_string(), JsonVal::String(skeleton));
        records.push(rec);
    }

    Ok(Value::from_records(records))
}

pub fn run_ast(session: &Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let Some(pattern) = args.first() else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "ast requires a symbol or pattern",
            None,
            Some("usage: ast <symbol> [path]".to_string()),
        ));
    };
    let root = match args.get(1) {
        Some(p) => resolve_path(session, p),
        None => session.cwd.clone(),
    };

    let walker = WalkBuilder::new(&root).build();
    let mut records = Vec::new();

    for entry_res in walker {
        let Ok(entry) = entry_res else {
            continue;
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let file_label = path
            .strip_prefix(&session.cwd)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            collect_rust_ast_matches(&bytes, &file_label, pattern, &mut records);
        } else {
            collect_line_ast_matches(&bytes, &file_label, pattern, &mut records);
        }
    }

    Ok(Value::from_records(records))
}

fn extract_skeleton_bytes(bytes: &[u8]) -> String {
    const PREFIXES: &[&[u8]] = &[
        b"pub fn ",
        b"pub async fn ",
        b"pub struct ",
        b"pub enum ",
        b"pub trait ",
        b"pub type ",
        b"impl ",
        b"fn ",
        b"async fn ",
        b"export ",
        b"class ",
        b"def ",
        b"func ",
    ];

    let mut out = String::new();
    for (idx, line) in bytes.split(|&b| b == b'\n').enumerate() {
        let trimmed = trim_ascii_bytes(line);
        if trimmed.len() < 3 || trimmed.starts_with(b"//") || trimmed.starts_with(b"#") {
            continue;
        }
        let head = &trimmed[..trimmed.len().min(24)];
        if PREFIXES.iter().any(|p| head.starts_with(p)) {
            let clean = trim_ascii_end_brace(trimmed);
            let h = hashline_bytes(clean);
            let s = String::from_utf8_lossy(clean);
            if !out.is_empty() {
                out.push_str(" | ");
            }
            let h_str = std::str::from_utf8(&h).unwrap_or("000");
            let _ = write!(out, "{}:{h_str} {s}", idx + 1);
        }
    }
    out
}

fn extract_rust_treesitter(bytes: &[u8]) -> String {
    with_rust_parser(|parser| {
        let Some(tree) = parser.parse(bytes, None) else {
            return String::new();
        };
        let mut out = String::new();
        let root = tree.root_node();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if let Some(sig) = rust_node_sig(child, bytes) {
                if !out.is_empty() {
                    out.push_str(" | ");
                }
                let line = child.start_position().row + 1;
                let h = hashline_bytes(sig.as_bytes());
                let h_str = std::str::from_utf8(&h).unwrap_or("000");
                let _ = write!(out, "{line}:{h_str} {sig}");
            }
        }
        out
    })
}

fn rust_node_sig(node: Node, bytes: &[u8]) -> Option<String> {
    let kind = node.kind();
    if !matches!(
        kind,
        "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item"
    ) {
        return None;
    }
    let slice = &bytes[node.start_byte()..node.end_byte()];
    let first_line = slice.split(|&b| b == b'\n').next().unwrap_or(slice);
    let clean = trim_ascii_end_brace(trim_ascii_bytes(first_line));
    Some(String::from_utf8_lossy(clean).to_string())
}

fn collect_rust_ast_matches(
    bytes: &[u8],
    file_label: &str,
    pattern: &str,
    records: &mut Vec<StructuredRecord>,
) {
    with_rust_parser(|parser| {
        let Some(tree) = parser.parse(bytes, None) else {
            return;
        };
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            let kind = node.kind();
            if matches!(
                kind,
                "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item"
            ) && let Some(sig) = rust_node_sig(node, bytes)
                && sig.contains(pattern)
            {
                let mut rec = StructuredRecord::new();
                rec.insert("file".to_string(), JsonVal::String(file_label.to_string()));
                rec.insert("line".to_string(), json!(node.start_position().row + 1));
                rec.insert("hash".to_string(), JsonVal::String(hashline_tag(&sig)));
                rec.insert("kind".to_string(), JsonVal::String(kind.to_string()));
                rec.insert("text".to_string(), JsonVal::String(sig));
                records.push(rec);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    });
}

fn collect_line_ast_matches(
    bytes: &[u8],
    file_label: &str,
    pattern: &str,
    records: &mut Vec<StructuredRecord>,
) {
    const MARKERS: &[(&[u8], &str)] = &[
        (b"class ", "class"),
        (b"def ", "def"),
        (b"func ", "func"),
        (b"function ", "function"),
    ];
    let pat_bytes = pattern.as_bytes();
    for (idx, line) in bytes.split(|&b| b == b'\n').enumerate() {
        let trimmed = trim_ascii_bytes(line);
        if memchr::memmem::find(trimmed, pat_bytes).is_none() {
            continue;
        }
        if let Some((_, kind)) = MARKERS
            .iter()
            .find(|(m, _)| memchr::memmem::find(trimmed, m).is_some())
        {
            let text = String::from_utf8_lossy(trimmed).to_string();
            let mut rec = StructuredRecord::new();
            rec.insert("file".to_string(), JsonVal::String(file_label.to_string()));
            rec.insert("line".to_string(), json!(idx + 1));
            rec.insert("hash".to_string(), JsonVal::String(hashline_tag(&text)));
            rec.insert("kind".to_string(), JsonVal::String((*kind).to_string()));
            rec.insert("text".to_string(), JsonVal::String(text));
            records.push(rec);
        }
    }
}

fn trim_ascii_bytes(mut b: &[u8]) -> &[u8] {
    while let Some(&first) = b.first() {
        if first.is_ascii_whitespace() {
            b = &b[1..];
        } else {
            break;
        }
    }
    while let Some(&last) = b.last() {
        if last.is_ascii_whitespace() {
            b = &b[..b.len() - 1];
        } else {
            break;
        }
    }
    b
}

fn trim_ascii_end_brace(mut b: &[u8]) -> &[u8] {
    b = trim_ascii_bytes(b);
    if b.ends_with(b"{") {
        b = trim_ascii_bytes(&b[..b.len() - 1]);
    }
    b
}

fn score_centrality(rel: &str, skeleton: &str) -> usize {
    let mut score = skeleton.as_bytes().iter().filter(|&&b| b == b'|').count() * 2 + 2;
    if rel.ends_with("Cargo.toml") || rel.ends_with("package.json") {
        score += 100;
    }
    if rel.ends_with("lib.rs")
        || rel.ends_with("main.rs")
        || rel.ends_with("mod.rs")
        || rel.ends_with("index.ts")
    {
        score += 50;
    }
    score
}

fn resolve_path(session: &Session, raw: &str) -> std::path::PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        session.cwd.join(p)
    }
}
