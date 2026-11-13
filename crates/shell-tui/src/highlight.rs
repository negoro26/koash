use std::path::Path;

use shell_builtins::is_builtin;
use shell_syntax::{TokenKind, WordPart, tokenize};

pub fn highlight_line(input: &str) -> String {
    let Ok(tokens) = tokenize(input) else {
        return input.to_string();
    };

    let mut out = String::with_capacity(input.len() + 32);
    let mut cursor = 0usize;
    let mut expect_cmd = true;

    for tok in tokens {
        if tok.kind == TokenKind::Eof {
            break;
        }
        if tok.span.start > cursor
            && let Some(gap) = input.get(cursor..tok.span.start)
        {
            out.push_str(gap);
        }
        let slice = tok.span.slice(input);
        match &tok.kind {
            TokenKind::Pipe
            | TokenKind::AndIf
            | TokenKind::OrIf
            | TokenKind::Semi
            | TokenKind::Amp => {
                out.push_str("\x1b[35m");
                out.push_str(slice);
                out.push_str("\x1b[0m");
                expect_cmd = true;
            }
            TokenKind::RedirectIn
            | TokenKind::RedirectOut
            | TokenKind::RedirectAppend
            | TokenKind::RedirectErrOut
            | TokenKind::RedirectErrAppend
            | TokenKind::RedirectErrToOut
            | TokenKind::RedirectBothOut => {
                out.push_str("\x1b[35m");
                out.push_str(slice);
                out.push_str("\x1b[0m");
            }
            TokenKind::Word(word) => {
                let is_quoted = word.parts.iter().any(|p| {
                    matches!(
                        p,
                        WordPart::SingleQuoted { .. } | WordPart::DoubleQuoted { .. }
                    )
                });

                if expect_cmd {
                    expect_cmd = false;
                    if command_exists(slice) {
                        out.push_str("\x1b[32m");
                    } else {
                        out.push_str("\x1b[31m");
                    }
                    out.push_str(slice);
                    out.push_str("\x1b[0m");
                } else if is_quoted {
                    out.push_str("\x1b[33m");
                    out.push_str(slice);
                    out.push_str("\x1b[0m");
                } else if slice.starts_with('-') {
                    out.push_str("\x1b[36m");
                    out.push_str(slice);
                    out.push_str("\x1b[0m");
                } else {
                    out.push_str(slice);
                }
            }
            _ => {
                out.push_str(slice);
            }
        }
        cursor = tok.span.end;
    }

    if cursor < input.len()
        && let Some(tail) = input.get(cursor..)
    {
        out.push_str(tail);
    }

    out
}

fn command_exists(name: &str) -> bool {
    if is_builtin(name) || matches!(name, "jobs" | "fg" | "bg" | "exit") {
        return true;
    }
    if name.contains('/') {
        return Path::new(name).is_file();
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(name).is_file() {
                return true;
            }
        }
    }
    false
}
