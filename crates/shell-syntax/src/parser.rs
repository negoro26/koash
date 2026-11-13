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

    fn is_sigil_cmd(words: &[Word]) -> bool {
        let Some(first) = words.first() else {
            return false;
        };
        matches!(first.parts.as_slice(), [WordPart::Literal { text, .. }] if text.len() > 1 && text.starts_with('@'))
    }

    /// The source of a sigil command (`@sql`, `@py`, `@js`) is code in another language, so its
    /// arguments are taken verbatim from `src` up to the end of the command. A lone word keeps
    /// its shell meaning: it is the quoted program and may be followed by redirections.
    fn take_sigil_source(&mut self) -> Option<Word> {
        let is_end = |kind: &TokenKind| {
            matches!(
                kind,
                TokenKind::Pipe
                    | TokenKind::Semi
                    | TokenKind::AndIf
                    | TokenKind::OrIf
                    | TokenKind::Amp
                    | TokenKind::Newline
                    | TokenKind::Eof
            )
        };
        let rest = &self.tokens[self.pos..];
        let len = rest.iter().take_while(|t| !is_end(&t.kind)).count();
        let (first, last) = (rest.first()?, rest.get(len.checked_sub(1)?)?);
        let lone_word = matches!(first.kind, TokenKind::Word(_))
            && rest
                .get(1)
                .is_none_or(|t| !matches!(t.kind, TokenKind::Word(_)));
        if lone_word {
            return None;
        }
        let span = first.span.merge(last.span);
        self.pos += len;
        Some(Word {
            parts: vec![WordPart::Literal {
                text: self.src.get(span.start..span.end).unwrap_or("").to_string(),
                span,
            }],
            span,
        })
    }

    /// The lexer reads `>=` as the redirect `>` followed by the word `=`. After `where <field>`
    /// that pair is a comparison operator, so glue it back together from the source text.
    fn comparison_operator(&mut self, op_span: Span) -> Word {
        let mut span = op_span;
        if let Some(Token {
            kind: TokenKind::Word(next),
            span: next_span,
        }) = self.tokens.get_mut(self.pos)
            && next_span.start == op_span.end
            && let Some(WordPart::Literal {
                text,
                span: lit_span,
            }) = next.parts.first_mut()
            && text.starts_with('=')
        {
            span = Span::new(op_span.start, op_span.end + 1);
            text.remove(0);
            *lit_span = Span::new(lit_span.start + 1, lit_span.end);
            if text.is_empty() {
                next.parts.remove(0);
            }
            next.span = Span::new(next.span.start + 1, next.span.end);
            *next_span = next.span;
            if next.parts.is_empty() {
                self.pos += 1;
            }
        }
        Word {
            parts: vec![WordPart::Literal {
                text: self.src.get(span.start..span.end).unwrap_or("").to_string(),
                span,
            }],
            span,
        }
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

        loop {
            if words.len() == 1
                && Self::is_sigil_cmd(&words)
                && let Some(code) = self.take_sigil_source()
            {
                overall_span = Some(overall_span.map_or(code.span, |s| s.merge(code.span)));
                words.push(code);
                continue;
            }
            let Some(tok) = self.peek() else { break };
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
                        let op = self.comparison_operator(tok_span);
                        overall_span = Some(overall_span.map_or(op.span, |s| s.merge(op.span)));
                        words.push(op);
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
                        let op = self.comparison_operator(tok_span);
                        overall_span = Some(overall_span.map_or(op.span, |s| s.merge(op.span)));
                        words.push(op);
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
                    if Self::is_where_cmd(&words) && words.len() == 2 {
                        let op = self.comparison_operator(tok_span);
                        overall_span = Some(overall_span.map_or(op.span, |s| s.merge(op.span)));
                        words.push(op);
                    } else {
                        let redir =
                            self.parse_redir_with_target(RedirectKind::StdoutAppend, tok_span)?;
                        overall_span = Some(match overall_span {
                            Some(s) => s.merge(redir.span),
                            None => redir.span,
                        });
                        redirections.push(redir);
                    }
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
