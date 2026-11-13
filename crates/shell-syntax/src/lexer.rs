use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ast::{Word, WordPart};
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
                b'$' => parts.push(self.lex_dollar()?),
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
                let name_start = self.pos;
                while let Some(b) = self.peek() {
                    if b == b'}' {
                        let name = self.src.get(name_start..self.pos).unwrap_or("").to_string();
                        self.pos += 1;
                        if name.is_empty()
                            || !name.bytes().all(|c| {
                                c.is_ascii_alphanumeric() || c == b'_' || c == b'?' || c == b'$'
                            })
                        {
                            return Err(SyntaxError::new(
                                "E_POSIX_PARAM_EXP",
                                "complex parameter expansion requires POSIX fallback",
                                Span::new(start, self.pos),
                                None,
                            ));
                        }
                        return Ok(WordPart::Var {
                            name,
                            span: Span::new(start, self.pos),
                        });
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

    fn lex_unquoted_literal(&mut self) -> Result<WordPart, SyntaxError> {
        let start = self.pos;
        let mut text = String::new();
        while let Some(b) = self.peek() {
            if Self::is_word_delimiter(b) || matches!(b, b'\'' | b'"' | b'$') {
                break;
            }
            if b == b'\\' {
                self.pos += 1;
                if self.peek().is_some() {
                    let ch = self.next_char();
                    if ch != '\n' {
                        text.push(ch);
                    }
                } else {
                    text.push('\\');
                }
                continue;
            }
            let ch = self.next_char();
            text.push(ch);
        }
        Ok(WordPart::Literal {
            text,
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
