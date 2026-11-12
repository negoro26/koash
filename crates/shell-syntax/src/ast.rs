use serde::{Deserialize, Serialize};

use crate::span::Span;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub items: Vec<ListItem>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListItem {
    pub pipeline: Pipeline,
    pub op: Option<ListOp>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListOp {
    Seq,
    And,
    Or,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    Simple(SimpleCommand),
    PosixFallback { raw: String, span: Span },
}

impl Command {
    pub fn span(&self) -> Span {
        match self {
            Command::Simple(c) => c.span,
            Command::PosixFallback { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirections: Vec<Redirection>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub parts: Vec<WordPart>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordPart {
    Literal { text: String, span: Span },
    SingleQuoted { text: String, span: Span },
    DoubleQuoted { parts: Vec<WordPart>, span: Span },
    Var { name: String, span: Span },
    Tilde { span: Span },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redirection {
    pub kind: RedirectKind,
    pub target: Option<Word>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RedirectKind {
    Stdin,
    StdoutTrunc,
    StdoutAppend,
    StderrTrunc,
    StderrAppend,
    StderrToStdout,
    StdoutAndStderrTrunc,
}
