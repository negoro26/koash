use std::fs;
use std::path::Path;

use glob::{MatchOptions, Pattern};

use crate::word_expand::{Kind, Sym};

const OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: false,
    require_literal_leading_dot: true,
};

pub(crate) fn has_meta(syms: &[Sym]) -> bool {
    syms.iter().any(|s| is_meta(*s))
}

fn is_meta(sym: Sym) -> bool {
    sym.kind != Kind::Quoted && matches!(sym.ch, '*' | '?' | '[')
}

pub(crate) fn expand(syms: &[Sym], cwd: &Path) -> Vec<String> {
    let absolute = syms.first().is_some_and(|s| s.ch == '/');
    let mut segments: Vec<&[Sym]> = syms.split(|s| s.ch == '/').collect();
    if absolute {
        segments.remove(0);
    }
    let dir_only = segments.len() > 1 && segments.last().is_some_and(|s| s.is_empty());
    if dir_only {
        segments.pop();
    }

    let mut candidates = vec![String::from(if absolute { "/" } else { "" })];
    let mut tail_is_literal = false;
    let last = segments.len().saturating_sub(1);
    for (i, segment) in segments.iter().enumerate() {
        let Some(pattern) = compile(segment) else {
            let literal: String = segment.iter().map(|s| s.ch).collect();
            for candidate in &mut candidates {
                *candidate = join(candidate, &literal);
            }
            tail_is_literal = i == last;
            continue;
        };
        tail_is_literal = false;
        let needs_dir = i != last || dir_only;
        let mut next = Vec::new();
        for candidate in &candidates {
            let dir = cwd.join(if candidate.is_empty() { "." } else { candidate });
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if pattern.matches_with(name, OPTIONS) && (!needs_dir || is_dir(&entry, &dir)) {
                    let mut name = name.to_owned();
                    if dir_only && i == last {
                        name.push('/');
                    }
                    names.push(name);
                }
            }
            names.sort_unstable();
            next.extend(names.iter().map(|name| join(candidate, name)));
        }
        candidates = next;
        if candidates.is_empty() {
            return candidates;
        }
    }

    if tail_is_literal {
        candidates.retain(|c| fs::symlink_metadata(cwd.join(c)).is_ok());
    }
    if dir_only && tail_is_literal {
        candidates.retain(|c| fs::metadata(cwd.join(c)).is_ok_and(|m| m.is_dir()));
        for candidate in &mut candidates {
            candidate.push('/');
        }
    }
    candidates
}

fn is_dir(entry: &fs::DirEntry, dir: &Path) -> bool {
    match entry.file_type() {
        Ok(ft) if ft.is_symlink() => {
            fs::metadata(dir.join(entry.file_name())).is_ok_and(|m| m.is_dir())
        }
        Ok(ft) => ft.is_dir(),
        Err(_) => false,
    }
}

fn join(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else if base == "/" {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn compile(segment: &[Sym]) -> Option<Pattern> {
    if !has_meta(segment) {
        return None;
    }
    let mut pattern = String::with_capacity(segment.len() + 2);
    let mut after_open = false;
    for &sym in segment {
        let active = sym.kind != Kind::Quoted;
        match sym.ch {
            '*' if active => {
                if !pattern.ends_with('*') {
                    pattern.push('*');
                }
            }
            '^' if active && after_open => pattern.push('!'),
            '*' | '?' | '[' if !active => {
                pattern.push('[');
                pattern.push(sym.ch);
                pattern.push(']');
            }
            ch => pattern.push(ch),
        }
        after_open = active && sym.ch == '[';
    }
    Pattern::new(&pattern).ok()
}
