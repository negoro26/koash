use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ast::{ParamOp, Word, WordPart};
use crate::span::Span;

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{message}")]
pub struct SyntaxError {
    pub code: String,
    pub message: String,
    pub span: Span,
    pub hint: Option<String>,
}

impl SyntaxError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        span: Span,
        hint: Option<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            span,
            hint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Word(Word),
    Pipe,
    PipeBoth,
    AndIf,
    OrIf,
    Semi,
    Amp,
    Newline,
    RedirectIn,
    RedirectOut,
    RedirectAppend,
    RedirectErrOut,
    RedirectErrAppend,
    RedirectErrToOut,
    RedirectBothOut,
    PosixConstruct,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src
            .get(self.pos..)
            .is_some_and(|rest| rest.starts_with(s))
    }

    fn skip_horizontal_ws_and_comments(&mut self) {
        loop {
            while let Some(b) = self.peek() {
                if b == b' ' || b == b'\t' || b == b'\r' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.peek() == Some(b'#') {
                while let Some(b) = self.peek() {
                    if b == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    pub fn tokenize_all(&mut self) -> Result<Vec<Token>, SyntaxError> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    pub fn next_token(&mut self) -> Result<Token, SyntaxError> {
        self.skip_horizontal_ws_and_comments();

        let start = self.pos;
        let Some(b) = self.peek() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span: Span::empty(start),
            });
        };

        if b == b'\n' {
            self.pos += 1;
            return Ok(Token {
                kind: TokenKind::Newline,
                span: Span::new(start, self.pos),
            });
        }

        if self.starts_with("2>&1") {
            self.pos += 4;
            return Ok(Token {
                kind: TokenKind::RedirectErrToOut,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with("2>>") {
            self.pos += 3;
            return Ok(Token {
                kind: TokenKind::RedirectErrAppend,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with("2>") {
            self.pos += 2;
            return Ok(Token {
                kind: TokenKind::RedirectErrOut,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with("&>") {
            self.pos += 2;
            return Ok(Token {
                kind: TokenKind::RedirectBothOut,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with("&&") {
            self.pos += 2;
            return Ok(Token {
                kind: TokenKind::AndIf,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with("||") {
            self.pos += 2;
            return Ok(Token {
                kind: TokenKind::OrIf,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with("|&") {
            self.pos += 2;
            return Ok(Token {
                kind: TokenKind::PipeBoth,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with(">>") {
            self.pos += 2;
            return Ok(Token {
                kind: TokenKind::RedirectAppend,
                span: Span::new(start, self.pos),
            });
        }
        if self.starts_with("<<") {
            self.pos += 2;
            return Ok(Token {
                kind: TokenKind::PosixConstruct,
                span: Span::new(start, self.pos),
            });
        }

        match b {
            b'|' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Pipe,
                    span: Span::new(start, self.pos),
                });
            }
            b'&' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Amp,
                    span: Span::new(start, self.pos),
                });
            }
            b';' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Semi,
                    span: Span::new(start, self.pos),
                });
            }
            b'<' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::RedirectIn,
                    span: Span::new(start, self.pos),
                });
            }
            b'>' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::RedirectOut,
                    span: Span::new(start, self.pos),
                });
            }
            b'(' | b')' | b'`' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::PosixConstruct,
                    span: Span::new(start, self.pos),
                });
            }
            _ => {}
        }

        let word = self.lex_word()?;
        Ok(Token {
            span: word.span,
            kind: TokenKind::Word(word),
        })
    }

    fn is_word_delimiter(b: u8) -> bool {
        matches!(
            b,
            b' ' | b'\t' | b'\r' | b'\n' | b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')' | b'`'
        )
    }

    fn lex_word(&mut self) -> Result<Word, SyntaxError> {
        let start = self.pos;
        let mut parts = Vec::new();

        if self.peek() == Some(b'~')
            && matches!(
                self.peek_at(1),
                None | Some(b'/')
                    | Some(b' ')
                    | Some(b'\t')
                    | Some(b'\n')
                    | Some(b';')
                    | Some(b'|')
                    | Some(b'&')
                    | Some(b'>')
                    | Some(b'<')
            )
        {
            self.pos += 1;
            parts.push(WordPart::Tilde {
                span: Span::new(start, self.pos),
            });
        }

        while let Some(b) = self.peek() {
            if Self::is_word_delimiter(b) {
                break;
            }
            match b {
                b'\'' => parts.push(self.lex_single_quoted()?),
                b'"' => parts.push(self.lex_double_quoted()?),
                b'$' if self.peek_at(1) == Some(b'\'') => parts.push(self.lex_ansi_c_quoted()?),
                b'$' => parts.push(self.lex_dollar()?),
                b'\\' => parts.extend(self.lex_escape()),
                _ => parts.push(self.lex_unquoted_literal()?),
            }
        }

        Ok(Word {
            parts,
            span: Span::new(start, self.pos),
        })
    }

    fn lex_single_quoted(&mut self) -> Result<WordPart, SyntaxError> {
        let start = self.pos;
        self.pos += 1;
        let content_start = self.pos;
        while let Some(b) = self.peek() {
            if b == b'\'' {
                let text = self
                    .src
                    .get(content_start..self.pos)
                    .unwrap_or("")
                    .to_string();
                self.pos += 1;
                return Ok(WordPart::SingleQuoted {
                    text,
                    span: Span::new(start, self.pos),
                });
            }
            self.pos += 1;
        }
        Err(SyntaxError::new(
            "E_UNCLOSED_SINGLE_QUOTE",
            "unterminated single-quoted string",
            Span::new(start, self.pos),
            Some("add a closing `'`".to_string()),
        ))
    }

    fn lex_ansi_c_quoted(&mut self) -> Result<WordPart, SyntaxError> {
        let start = self.pos;
        self.pos += 2;
        let mut text = String::new();
        while let Some(b) = self.peek() {
            if b == b'\'' {
                self.pos += 1;
                return Ok(WordPart::SingleQuoted {
                    text,
                    span: Span::new(start, self.pos),
                });
            }
            if b != b'\\' {
                text.push(self.next_char());
                continue;
            }
            self.pos += 1;
            let Some(esc) = self.peek() else {
                break;
            };
            match esc {
                b'a' => {
                    text.push('\x07');
                    self.pos += 1;
                }
                b'b' => {
                    text.push('\x08');
                    self.pos += 1;
                }
                b'e' | b'E' => {
                    text.push('\x1b');
                    self.pos += 1;
                }
                b'f' => {
                    text.push('\x0c');
                    self.pos += 1;
                }
                b'n' => {
                    text.push('\n');
                    self.pos += 1;
                }
                b'r' => {
                    text.push('\r');
                    self.pos += 1;
                }
                b't' => {
                    text.push('\t');
                    self.pos += 1;
                }
                b'v' => {
                    text.push('\x0b');
                    self.pos += 1;
                }
                b'\\' | b'\'' | b'"' | b'?' => {
                    text.push(esc as char);
                    self.pos += 1;
                }
                b'0'..=b'7' => {
                    let mut val = 0u32;
                    for _ in 0..3 {
                        if let Some(d @ b'0'..=b'7') = self.peek() {
                            val = (val << 3) | u32::from(d - b'0');
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    if let Some(ch) = char::from_u32(val & 0xff) {
                        text.push(ch);
                    }
                }
                b'c' => {
                    self.pos += 1;
                    match self.peek() {
                        Some(ctl) if ctl.is_ascii() && ctl != b'\'' => {
                            self.pos += 1;
                            if ctl == b'\\' && self.peek() == Some(b'\\') {
                                self.pos += 1;
                            }
                            text.push(if ctl == b'?' {
                                '\x7f'
                            } else {
                                char::from(ctl & 0x1f)
                            });
                        }
                        _ => text.push_str("\\c"),
                    }
                }
                b'x' => {
                    self.pos += 1;
                    let (val, count) = self.take_hex(2);
                    if count > 0 {
                        if let Some(ch) = char::from_u32(val) {
                            text.push(ch);
                        }
                    } else {
                        text.push_str("\\x");
                    }
                }
                b'u' | b'U' => {
                    let max_digits = if esc == b'u' { 4 } else { 8 };
                    self.pos += 1;
                    let (val, count) = self.take_hex(max_digits);
                    if count > 0
                        && let Some(ch) = char::from_u32(val)
                    {
                        text.push(ch);
                    } else {
                        text.push('\\');
                        text.push(esc as char);
                    }
                }
                _ => {
                    text.push('\\');
                    text.push(self.next_char());
                }
            }
        }
        Err(SyntaxError::new(
            "E_UNCLOSED_SINGLE_QUOTE",
            "unterminated `$'...'` string",
            Span::new(start, self.pos),
            Some("add a closing `'`".to_string()),
        ))
    }

    fn take_hex(&mut self, max_digits: usize) -> (u32, usize) {
        let mut val = 0u32;
        let mut count = 0usize;
        while count < max_digits {
            let Some(b) = self.peek() else { break };
            let digit = match b {
                b'0'..=b'9' => u32::from(b - b'0'),
                b'a'..=b'f' => u32::from(b - b'a' + 10),
                b'A'..=b'F' => u32::from(b - b'A' + 10),
                _ => break,
            };
            val = (val << 4) | digit;
            count += 1;
            self.pos += 1;
        }
        (val, count)
    }

    fn lex_double_quoted(&mut self) -> Result<WordPart, SyntaxError> {
        let start = self.pos;
        self.pos += 1;
        let mut parts = Vec::new();
        let mut lit_buf = String::new();
        let mut lit_start = self.pos;

        while let Some(b) = self.peek() {
            if b == b'"' {
                if !lit_buf.is_empty() {
                    parts.push(WordPart::Literal {
                        text: std::mem::take(&mut lit_buf),
                        span: Span::new(lit_start, self.pos),
                    });
                }
                self.pos += 1;
                return Ok(WordPart::DoubleQuoted {
                    parts,
                    span: Span::new(start, self.pos),
                });
            }
            if b == b'\\' {
                self.pos += 1;
                if let Some(esc) = self.peek() {
                    match esc {
                        b'"' | b'\\' | b'$' | b'`' => {
                            lit_buf.push(esc as char);
                            self.pos += 1;
                        }
                        b'\n' => {
                            self.pos += 1;
                        }
                        _ => {
                            lit_buf.push('\\');
                            let ch = self.next_char();
                            lit_buf.push(ch);
                        }
                    }
                } else {
                    return Err(SyntaxError::new(
                        "E_UNCLOSED_DOUBLE_QUOTE",
                        "unterminated double-quoted string after escape",
                        Span::new(start, self.pos),
                        Some("add a closing `\"`".to_string()),
                    ));
                }
                continue;
            }
            if b == b'$' {
                if !lit_buf.is_empty() {
                    parts.push(WordPart::Literal {
                        text: std::mem::take(&mut lit_buf),
                        span: Span::new(lit_start, self.pos),
                    });
                }
                parts.push(self.lex_dollar()?);
                lit_start = self.pos;
                continue;
            }
            if b == b'`' {
                return Err(SyntaxError::new(
                    "E_POSIX_BACKTICK",
                    "backtick command substitution requires POSIX fallback",
                    Span::new(self.pos, self.pos + 1),
                    None,
                ));
            }
            let ch = self.next_char();
            lit_buf.push(ch);
        }

        Err(SyntaxError::new(
            "E_UNCLOSED_DOUBLE_QUOTE",
            "unterminated double-quoted string",
            Span::new(start, self.pos),
            Some("add a closing `\"`".to_string()),
        ))
    }

    fn lex_dollar(&mut self) -> Result<WordPart, SyntaxError> {
        let start = self.pos;
        self.pos += 1;

        match self.peek() {
            Some(b'(') => Err(SyntaxError::new(
                "E_POSIX_SUBST",
                "command substitution `$(...)` requires POSIX fallback",
                Span::new(start, self.pos + 1),
                None,
            )),
            Some(b'{') => {
                self.pos += 1;
                let inner_start = self.pos;
                while let Some(b) = self.peek() {
                    if b == b'}' {
                        let inner = self.src.get(inner_start..self.pos).unwrap_or("");
                        self.pos += 1;
                        let span = Span::new(start, self.pos);
                        if let Some(part) = parse_brace_param(inner, span) {
                            return Ok(part);
                        }
                        return Err(SyntaxError::new(
                            "E_POSIX_PARAM_EXP",
                            "complex parameter expansion requires POSIX fallback",
                            span,
                            None,
                        ));
                    }
                    self.pos += 1;
                }
                Err(SyntaxError::new(
                    "E_UNCLOSED_BRACE",
                    "unterminated `${...}` variable expansion",
                    Span::new(start, self.pos),
                    Some("add a closing `}`".to_string()),
                ))
            }
            Some(b'?') | Some(b'$') | Some(b'#') | Some(b'!') | Some(b'@') | Some(b'*') => {
                let ch = self.next_char();
                Ok(WordPart::Var {
                    name: ch.to_string(),
                    span: Span::new(start, self.pos),
                })
            }
            Some(b) if b.is_ascii_digit() => {
                let ch = self.next_char();
                Ok(WordPart::Var {
                    name: ch.to_string(),
                    span: Span::new(start, self.pos),
                })
            }
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => {
                let name_start = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphanumeric() || c == b'_' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let name = self.src.get(name_start..self.pos).unwrap_or("").to_string();
                Ok(WordPart::Var {
                    name,
                    span: Span::new(start, self.pos),
                })
            }
            _ => Ok(WordPart::Literal {
                text: "$".to_string(),
                span: Span::new(start, self.pos),
            }),
        }
    }

    fn lex_escape(&mut self) -> Option<WordPart> {
        let start = self.pos;
        self.pos += 1;
        if self.peek().is_none() {
            return Some(WordPart::Literal {
                text: "\\".to_string(),
                span: Span::new(start, self.pos),
            });
        }
        let ch = self.next_char();
        (ch != '\n').then_some(WordPart::Escaped {
            ch,
            span: Span::new(start, self.pos),
        })
    }

    fn lex_unquoted_literal(&mut self) -> Result<WordPart, SyntaxError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if Self::is_word_delimiter(b) || matches!(b, b'\'' | b'"' | b'$' | b'\\') {
                break;
            }
            self.next_char();
        }
        Ok(WordPart::Literal {
            text: self.src.get(start..self.pos).unwrap_or("").to_string(),
            span: Span::new(start, self.pos),
        })
    }

    fn next_char(&mut self) -> char {
        let rest = self.src.get(self.pos..).unwrap_or("");
        if let Some(ch) = rest.chars().next() {
            self.pos += ch.len_utf8();
            ch
        } else {
            self.pos = self.bytes.len();
            '\0'
        }
    }
}

fn is_valid_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'?' || c == b'$')
}

fn parse_brace_param(inner: &str, span: Span) -> Option<WordPart> {
    if is_valid_var_name(inner) {
        return Some(WordPart::Var {
            name: inner.to_string(),
            span,
        });
    }
    if let Some(rest) = inner.strip_prefix('#')
        && is_valid_var_name(rest)
    {
        return Some(WordPart::ParamExp {
            name: rest.to_string(),
            op: ParamOp::Len,
            word: None,
            span,
        });
    }
    let name_end = inner
        .bytes()
        .position(|b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'?' || b == b'$'))?;
    let name = &inner[..name_end];
    if name.is_empty() {
        return None;
    }
    let rest = &inner[name_end..];
    let (op, word) = if let Some(w) = rest.strip_prefix(":-") {
        (ParamOp::Default { colon: true }, w)
    } else if let Some(w) = rest.strip_prefix('-') {
        (ParamOp::Default { colon: false }, w)
    } else if let Some(w) = rest.strip_prefix(":+") {
        (ParamOp::Alt { colon: true }, w)
    } else {
        let w = rest.strip_prefix('+')?;
        (ParamOp::Alt { colon: false }, w)
    };
    let plain = !word.contains(['$', '`', '\\', '\'', '"']) && !word.starts_with('~');
    if !plain {
        return None;
    }
    Some(WordPart::ParamExp {
        name: name.to_string(),
        op,
        word: Some(word.to_string()),
        span,
    })
}

pub fn line_continues(line: &str) -> bool {
    let (mut single, mut double) = (false, false);
    let mut prev: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '\\' if !single => {
                if chars.next().is_none() {
                    return true;
                }
            }
            '#' if !single
                && !double
                && prev.is_none_or(|p| p.is_whitespace() || ";&|()".contains(p)) =>
            {
                return false;
            }
            _ => {}
        }
        prev = Some(c);
    }
    false
}
