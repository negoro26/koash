use std::borrow::Cow;

use crate::ast::WordPart;
use crate::lexer::{Lexer, TokenKind};

const KEEP_COMMAND_POSITION: &[&str] = &[
    "!", "time", "if", "then", "else", "elif", "while", "until", "do", "{",
];

struct Scan {
    text: String,
    next_eligible: bool,
}

struct Expander<L> {
    lookup: L,
    active: Vec<String>,
}

pub fn expand_aliases<'a>(src: &'a str, lookup: impl Fn(&str) -> Option<String>) -> Cow<'a, str> {
    let mut expander = Expander {
        lookup,
        active: Vec::new(),
    };
    let scan = expander.scan(src);
    if scan.text == src {
        Cow::Borrowed(src)
    } else {
        Cow::Owned(scan.text)
    }
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn assignment_shaped(text: &str) -> bool {
    let Some((name, _)) = text.split_once('=') else {
        return false;
    };
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b == b'_' || b.is_ascii_alphabetic())
        && bytes.all(|b| b == b'_' || b.is_ascii_alphanumeric())
}

impl<L: Fn(&str) -> Option<String>> Expander<L> {
    fn scan(&mut self, src: &str) -> Scan {
        let mut lexer = Lexer::new(src);
        let mut text = String::with_capacity(src.len());
        let mut copied = 0;
        let mut eligible = true;
        let mut redirect_target = false;
        loop {
            let Ok(token) = lexer.next_token() else {
                eligible = false;
                break;
            };
            let span = token.span;
            match token.kind {
                TokenKind::Eof => break,
                TokenKind::Pipe
                | TokenKind::AndIf
                | TokenKind::OrIf
                | TokenKind::Semi
                | TokenKind::Amp
                | TokenKind::Newline => {
                    eligible = true;
                    redirect_target = false;
                }
                TokenKind::RedirectIn
                | TokenKind::RedirectOut
                | TokenKind::RedirectAppend
                | TokenKind::RedirectErrOut
                | TokenKind::RedirectErrAppend
                | TokenKind::RedirectBothOut => redirect_target = true,
                TokenKind::RedirectErrToOut => {}
                TokenKind::PosixConstruct => match span.slice(src) {
                    "(" => eligible = true,
                    "<<" => {
                        eligible = false;
                        break;
                    }
                    _ => eligible = false,
                },
                TokenKind::Word(word) => {
                    if std::mem::take(&mut redirect_target) || !eligible {
                        continue;
                    }
                    let raw = span.slice(src);
                    let name = match word.parts.as_slice() {
                        [WordPart::Literal { text, .. }] if text == raw => text.as_str(),
                        _ => {
                            eligible = false;
                            continue;
                        }
                    };
                    if assignment_shaped(name) || KEEP_COMMAND_POSITION.contains(&name) {
                        continue;
                    }
                    let value =
                        (self.lookup)(name).filter(|_| !self.active.iter().any(|a| a == name));
                    let Some(value) = value else {
                        eligible = false;
                        continue;
                    };
                    text.push_str(&src[copied..span.start]);
                    self.active.push(name.to_string());
                    let inner = self.scan(&value);
                    self.active.pop();
                    eligible = inner.next_eligible || inner.text.ends_with(is_blank);
                    text.push_str(&inner.text);
                    copied = span.end;
                }
            }
        }
        text.push_str(&src[copied..]);
        Scan {
            text,
            next_eligible: eligible,
        }
    }
}
