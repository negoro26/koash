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
    let mut prefix_literal = true;
    let last = segments.len().saturating_sub(1);
    for (i, segment) in segments.iter().enumerate() {
        if is_globstar(segment) {
            tail_is_literal = false;
            if i != last && is_globstar(segments[i + 1]) {
                continue;
            }
            let mut next = Vec::new();
            for candidate in &candidates {
                if i == last {
                    globstar_tail(cwd, candidate, dir_only, prefix_literal, &mut next);
                } else {
                    globstar_dirs(cwd, candidate, !candidate.is_empty(), &mut next);
                }
            }
            candidates = next;
            prefix_literal = false;
            if candidates.is_empty() {
                return candidates;
            }
            continue;
        }
        let Some(pattern) = compile(segment) else {
            let literal: String = segment.iter().map(|s| s.ch).collect();
            for candidate in &mut candidates {
                *candidate = join(candidate, &literal);
            }
            tail_is_literal = i == last;
            continue;
        };
        tail_is_literal = false;
        prefix_literal = false;
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
                if pattern.matches(name) && (!needs_dir || is_dir(&entry, &dir)) {
                    let mut name = name.to_owned();
                    if dir_only && i == last {
                        name.push('/');
                    }
                    names.push(name);
                }
            }
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
    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

fn is_globstar(segment: &[Sym]) -> bool {
    segment.len() == 2
        && segment
            .iter()
            .all(|s| s.kind != Kind::Quoted && s.ch == '*')
}

fn globstar_dirs(cwd: &Path, base: &str, symlink_leaves: bool, out: &mut Vec<String>) {
    let start = cwd.join(if base.is_empty() { "." } else { base });
    if !fs::metadata(&start).is_ok_and(|m| m.is_dir()) {
        return;
    }
    let mut stack = vec![base.to_string()];
    while let Some(dir) = stack.pop() {
        out.push(dir.clone());
        let dir_path = cwd.join(if dir.is_empty() { "." } else { &dir });
        let Ok(entries) = fs::read_dir(&dir_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str().filter(|n| !n.starts_with('.')) else {
                continue;
            };
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(join(&dir, name)),
                Ok(ft) if ft.is_symlink() && symlink_leaves && is_dir(&entry, &dir_path) => {
                    out.push(join(&dir, name));
                }
                _ => {}
            }
        }
    }
}

fn globstar_tail(
    cwd: &Path,
    base: &str,
    dir_only: bool,
    prefix_literal: bool,
    out: &mut Vec<String>,
) {
    let mut dirs = Vec::new();
    globstar_dirs(cwd, base, dir_only, &mut dirs);
    if dir_only {
        out.extend(
            dirs.into_iter()
                .filter(|dir| !dir.is_empty())
                .map(|dir| if dir == "/" { dir } else { format!("{dir}/") }),
        );
        return;
    }
    if !dirs.is_empty() && !base.is_empty() {
        out.push(if prefix_literal && base != "/" {
            format!("{base}/")
        } else {
            base.to_string()
        });
    }
    for dir in &dirs {
        let dir_path = cwd.join(if dir.is_empty() { "." } else { dir });
        let Ok(entries) = fs::read_dir(&dir_path) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str().filter(|n| !n.starts_with('.')) {
                out.push(join(dir, name));
            }
        }
    }
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

struct Compiled {
    pattern: Pattern,
    prefix: String,
    suffix: String,
}

impl Compiled {
    fn matches(&self, name: &str) -> bool {
        name.len() >= self.prefix.len() + self.suffix.len()
            && name.starts_with(&self.prefix)
            && name.ends_with(&self.suffix)
            && self.pattern.matches_with(name, OPTIONS)
    }
}

fn compile(segment: &[Sym]) -> Option<Compiled> {
    let first = segment.iter().position(|s| is_meta(*s))?;
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
    let has_class = segment
        .iter()
        .any(|s| s.kind != Kind::Quoted && s.ch == '[');
    let last = segment.iter().rposition(|s| is_meta(*s))?;
    let suffix = if has_class {
        &[][..]
    } else {
        &segment[last + 1..]
    };
    Some(Compiled {
        pattern: Pattern::new(&pattern).ok()?,
        prefix: segment[..first].iter().map(|s| s.ch).collect(),
        suffix: suffix.iter().map(|s| s.ch).collect(),
    })
}
