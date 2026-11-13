use crate::ast::{
    Assignment, Command, ListItem, ListOp, Pipeline, Program, RedirectKind, Redirection,
    SimpleCommand, Word, WordPart,
};
use crate::lexer::{Lexer, SyntaxError, Token, TokenKind};
use crate::span::Span;

const POSIX_KEYWORDS: &[&str] = &["if", "for", "while", "until", "case", "{", "function"];

pub struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
}

impl<'a> Parser<'a> {
    pub fn parse(src: &'a str) -> Result<Program, SyntaxError> {
        let mut lexer = Lexer::new(src);
        let tokens = match lexer.tokenize_all() {
            Ok(toks) => toks,
            Err(err)
                if err.code == "E_POSIX_SUBST"
                    || err.code == "E_POSIX_BACKTICK"
                    || err.code == "E_POSIX_PARAM_EXP" =>
            {
                return Ok(Self::posix_fallback_program(src));
            }
            Err(err) => return Err(err),
        };

        if tokens.iter().any(|t| t.kind == TokenKind::PosixConstruct) {
            return Ok(Self::posix_fallback_program(src));
        }

        let mut parser = Self {
            src,
            tokens,
            pos: 0,
        };
        parser.parse_program()
    }

    fn posix_fallback_program(src: &str) -> Program {
        let span = Span::new(0, src.len());
        Program {
            items: vec![ListItem {
                pipeline: Pipeline {
                    commands: vec![Command::PosixFallback {
                        raw: src.to_string(),
                        span,
                    }],
                    span,
                },
                op: None,
                span,
            }],
            span,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos);
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek().map(|t| &t.kind), Some(TokenKind::Newline)) {
            self.pos += 1;
        }
    }

    fn is_at_eof(&self) -> bool {
        matches!(self.peek().map(|t| &t.kind), None | Some(TokenKind::Eof))
    }

    fn parse_program(&mut self) -> Result<Program, SyntaxError> {
        let mut items = Vec::new();
        self.skip_newlines();

        while !self.is_at_eof() {
            let pipeline = self.parse_pipeline()?;

            if let Some(Command::Simple(cmd)) = pipeline.commands.first()
                && let Some(first_word) = cmd.words.first()
                && Self::is_plain_keyword(first_word)
            {
                return Ok(Self::posix_fallback_program(self.src));
            }

            let (op, op_span) = match self.peek().map(|t| (&t.kind, t.span)) {
                Some((TokenKind::Semi, sp)) => {
                    self.advance();
                    (Some(ListOp::Seq), Some(sp))
                }
                Some((TokenKind::Newline, sp)) => {
                    self.advance();
                    (Some(ListOp::Seq), Some(sp))
                }
                Some((TokenKind::AndIf, sp)) => {
                    self.advance();
                    (Some(ListOp::And), Some(sp))
                }
                Some((TokenKind::OrIf, sp)) => {
                    self.advance();
                    (Some(ListOp::Or), Some(sp))
                }
                Some((TokenKind::Amp, sp)) => {
                    self.advance();
                    (Some(ListOp::Background), Some(sp))
                }
                _ => (None, None),
            };

            let item_span = match op_span {
                Some(sp) => pipeline.span.merge(sp),
                None => pipeline.span,
            };

            items.push(ListItem {
                pipeline,
                op,
                span: item_span,
            });

            self.skip_newlines();

            if matches!(op, Some(ListOp::And) | Some(ListOp::Or)) && self.is_at_eof() {
                let err_span = op_span.unwrap_or_else(|| Span::empty(self.src.len()));
                return Err(SyntaxError::new(
                    "E_EXPECTED_COMMAND",
                    "expected command after logical operator",
                    err_span,
                    Some("provide a command after `&&` or `||`".to_string()),
                ));
            }
        }

        let span = match (items.first(), items.last()) {
            (Some(first), Some(last)) => first.span.merge(last.span),
            _ => Span::empty(0),
        };

        Ok(Program { items, span })
    }

    fn is_plain_keyword(word: &Word) -> bool {
        if word.parts.len() != 1 {
            return false;
        }
        if let Some(WordPart::Literal { text, .. }) = word.parts.first() {
            POSIX_KEYWORDS.contains(&text.as_str())
        } else {
            false
        }
    }

    fn is_where_cmd(words: &[Word]) -> bool {
        let Some(first) = words.first() else {
            return false;
        };
        first.parts.len() == 1
            && matches!(first.parts.first(), Some(WordPart::Literal { text, .. }) if text == "where")
    }

    fn parse_pipeline(&mut self) -> Result<Pipeline, SyntaxError> {
        let first = self.parse_command()?;
        let mut span = first.span();
        let mut commands = vec![first];

        while matches!(self.peek().map(|t| &t.kind), Some(TokenKind::Pipe)) {
            let pipe_span = self.peek().map(|t| t.span).unwrap_or(span);
            self.advance();
            self.skip_newlines();
            if self.is_at_eof()
                || matches!(
                    self.peek().map(|t| &t.kind),
                    Some(
                        TokenKind::Pipe
                            | TokenKind::Semi
                            | TokenKind::AndIf
                            | TokenKind::OrIf
                            | TokenKind::Amp
                    )
                )
            {
                return Err(SyntaxError::new(
                    "E_EXPECTED_PIPE_CMD",
                    "expected command after `|`",
                    pipe_span,
                    Some("provide a command on the right-hand side of `|`".to_string()),
                ));
            }
            let next_cmd = self.parse_command()?;
            span = span.merge(next_cmd.span());
            commands.push(next_cmd);
        }

        Ok(Pipeline { commands, span })
    }

    fn parse_command(&mut self) -> Result<Command, SyntaxError> {
        let mut assignments = Vec::new();
        let mut words = Vec::new();
        let mut redirections = Vec::new();
        let mut overall_span: Option<Span> = None;

        while let Some(tok) = self.peek() {
            let tok_span = tok.span;
            match &tok.kind {
                TokenKind::Word(word) => {
                    let word = word.clone();
                    self.advance();
                    overall_span = Some(match overall_span {
                        Some(s) => s.merge(word.span),
                        None => word.span,
                    });
                    if words.is_empty()
                        && let Some(assign) = Self::try_split_assignment(&word)
                    {
                        assignments.push(assign);
                    } else {
                        words.push(word);
                    }
                }
                TokenKind::RedirectIn => {
                    self.advance();
                    if Self::is_where_cmd(&words) && words.len() == 2 {
                        words.push(Word {
                            parts: vec![WordPart::Literal {
                                text: "<".to_string(),
                                span: tok_span,
                            }],
                            span: tok_span,
                        });
                        overall_span = Some(overall_span.map_or(tok_span, |s| s.merge(tok_span)));
                    } else {
                        let redir = self.parse_redir_with_target(RedirectKind::Stdin, tok_span)?;
                        overall_span = Some(match overall_span {
                            Some(s) => s.merge(redir.span),
                            None => redir.span,
                        });
                        redirections.push(redir);
                    }
                }
                TokenKind::RedirectOut => {
                    self.advance();
                    if Self::is_where_cmd(&words) && words.len() == 2 {
                        words.push(Word {
                            parts: vec![WordPart::Literal {
                                text: ">".to_string(),
                                span: tok_span,
                            }],
                            span: tok_span,
                        });
                        overall_span = Some(overall_span.map_or(tok_span, |s| s.merge(tok_span)));
                    } else {
                        let redir =
                            self.parse_redir_with_target(RedirectKind::StdoutTrunc, tok_span)?;
                        overall_span = Some(match overall_span {
                            Some(s) => s.merge(redir.span),
                            None => redir.span,
                        });
                        redirections.push(redir);
                    }
                }
                TokenKind::RedirectAppend => {
                    self.advance();
                    let redir =
                        self.parse_redir_with_target(RedirectKind::StdoutAppend, tok_span)?;
                    overall_span = Some(match overall_span {
                        Some(s) => s.merge(redir.span),
                        None => redir.span,
                    });
                    redirections.push(redir);
                }
                TokenKind::RedirectErrOut => {
                    self.advance();
                    let redir =
                        self.parse_redir_with_target(RedirectKind::StderrTrunc, tok_span)?;
                    overall_span = Some(match overall_span {
                        Some(s) => s.merge(redir.span),
                        None => redir.span,
                    });
                    redirections.push(redir);
                }
                TokenKind::RedirectErrAppend => {
                    self.advance();
                    let redir =
                        self.parse_redir_with_target(RedirectKind::StderrAppend, tok_span)?;
                    overall_span = Some(match overall_span {
                        Some(s) => s.merge(redir.span),
                        None => redir.span,
                    });
                    redirections.push(redir);
                }
                TokenKind::RedirectBothOut => {
                    self.advance();
                    let redir =
                        self.parse_redir_with_target(RedirectKind::StdoutAndStderrTrunc, tok_span)?;
                    overall_span = Some(match overall_span {
                        Some(s) => s.merge(redir.span),
                        None => redir.span,
                    });
                    redirections.push(redir);
                }
                TokenKind::RedirectErrToOut => {
                    self.advance();
                    let redir = Redirection {
                        kind: RedirectKind::StderrToStdout,
                        target: None,
                        span: tok_span,
                    };
                    overall_span = Some(match overall_span {
                        Some(s) => s.merge(redir.span),
                        None => redir.span,
                    });
                    redirections.push(redir);
                }
                _ => break,
            }
        }

        let Some(span) = overall_span else {
            let err_span = self
                .peek()
                .map(|t| t.span)
                .unwrap_or_else(|| Span::empty(self.src.len()));
            return Err(SyntaxError::new(
                "E_EXPECTED_COMMAND",
                "expected a command",
                err_span,
                None,
            ));
        };

        Ok(Command::Simple(SimpleCommand {
            assignments,
            words,
            redirections,
            span,
        }))
    }

    fn parse_redir_with_target(
        &mut self,
        kind: RedirectKind,
        op_span: Span,
    ) -> Result<Redirection, SyntaxError> {
        let Some(tok) = self.peek() else {
            return Err(SyntaxError::new(
                "E_EXPECTED_REDIR_TARGET",
                "expected filename after redirection operator",
                op_span,
                Some("provide a target file path".to_string()),
            ));
        };
        if let TokenKind::Word(target) = &tok.kind {
            let target = target.clone();
            let span = op_span.merge(target.span);
            self.advance();
            Ok(Redirection {
                kind,
                target: Some(target),
                span,
            })
        } else {
            Err(SyntaxError::new(
                "E_EXPECTED_REDIR_TARGET",
                "expected filename after redirection operator",
                tok.span,
                Some("provide a target file path".to_string()),
            ))
        }
    }

    fn try_split_assignment(word: &Word) -> Option<Assignment> {
        let first = word.parts.first()?;
        let WordPart::Literal { text, span } = first else {
            return None;
        };
        let eq_idx = text.find('=')?;
        let name = &text[..eq_idx];
        if name.is_empty()
            || !name.bytes().enumerate().all(|(i, b)| {
                (i == 0 && (b.is_ascii_alphabetic() || b == b'_'))
                    || (i > 0 && (b.is_ascii_alphanumeric() || b == b'_'))
            })
        {
            return None;
        }
        let rest_lit = &text[eq_idx + 1..];
        let mut val_parts = Vec::new();
        let val_start = span.start + eq_idx + 1;
        if !rest_lit.is_empty() {
            val_parts.push(WordPart::Literal {
                text: rest_lit.to_string(),
                span: Span::new(val_start, span.end),
            });
        }
        val_parts.extend(word.parts.iter().skip(1).cloned());
        Some(Assignment {
            name: name.to_string(),
            value: Word {
                parts: val_parts,
                span: Span::new(val_start, word.span.end),
            },
            span: word.span,
        })
    }
}
