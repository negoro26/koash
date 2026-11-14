use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::OnceLock;

use shell_builtins::{REGISTRY, lookup};

static PATH_EXEC_CACHE: OnceLock<Vec<String>> = OnceLock::new();

fn cached_path_executables() -> &'static [String] {
    PATH_EXEC_CACHE.get_or_init(|| {
        let mut bins = Vec::new();
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let Ok(meta) = entry.metadata() else {
                        continue;
                    };
                    if meta.is_file() && (meta.permissions().mode() & 0o111) != 0 {
                        bins.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
        }
        bins.sort();
        bins.dedup();
        bins
    })
}

pub fn complete_line(input: &str, cursor: usize, cwd: &Path) -> Vec<String> {
    let prefix = input.get(..cursor).unwrap_or(input);
    let last_segment = prefix.rsplit('|').next().unwrap_or(prefix);
    let ends_with_space = last_segment.ends_with(char::is_whitespace);
    let words: Vec<&str> = last_segment.split_whitespace().collect();

    let current_word = if ends_with_space {
        ""
    } else {
        words.last().copied().unwrap_or("")
    };

    let cmd_pos = if ends_with_space {
        words.is_empty()
    } else {
        words.len() <= 1
    };

    if cmd_pos {
        return complete_commands(current_word);
    }

    let cmd_name = words.first().copied().unwrap_or("");
    if current_word.starts_with('-')
        && let Some(builtin) = lookup(cmd_name)
    {
        let mut matches: Vec<String> = builtin
            .signature()
            .flags
            .iter()
            .flat_map(|f| f.spellings())
            .filter(|spelling| spelling.starts_with(current_word))
            .collect();
        if !matches.is_empty() {
            matches.sort();
            return matches;
        }
    }

    if matches!(cmd_name, "select" | "where") {
        let upstream_cmd = prefix
            .rsplit('|')
            .nth(1)
            .and_then(|seg| seg.split_whitespace().next());
        let mut cols = Vec::new();
        if let Some(up) = upstream_cmd
            && let Some(builtin) = lookup(up)
        {
            for col in builtin.signature().column_names() {
                if col.starts_with(current_word) {
                    cols.push(col.to_string());
                }
            }
        }
        if !cols.is_empty() {
            cols.sort();
            cols.dedup();
            return cols;
        }
    }

    complete_paths(current_word, cwd)
}

fn complete_commands(prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for builtin in REGISTRY {
        let name = builtin.signature().name;
        if name.starts_with(prefix) {
            out.push(name.to_string());
        }
    }
    for bin in cached_path_executables() {
        if bin.starts_with(prefix) {
            out.push(bin.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn complete_paths(prefix: &str, cwd: &Path) -> Vec<String> {
    let (dir_part, file_prefix) = match prefix.rfind('/') {
        Some(idx) => (&prefix[..=idx], &prefix[idx + 1..]),
        None => ("", prefix),
    };

    let search_dir = if dir_part.is_empty() {
        cwd.to_path_buf()
    } else if Path::new(dir_part).is_absolute() {
        Path::new(dir_part).to_path_buf()
    } else {
        cwd.join(dir_part)
    };

    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(file_prefix) {
                let mut candidate = format!("{dir_part}{name}");
                if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                    candidate.push('/');
                }
                out.push(candidate);
            }
        }
    }
    out.sort();
    out
}
