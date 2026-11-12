pub mod ast;
pub mod lexer;
pub mod parser;
pub mod span;

pub use ast::{
    Assignment, Command, ListItem, ListOp, Pipeline, Program, RedirectKind, Redirection,
    SimpleCommand, Word, WordPart,
};
pub use lexer::{Lexer, SyntaxError, Token, TokenKind};
pub use parser::Parser;
pub use span::Span;

pub fn parse(input: &str) -> Result<Program, SyntaxError> {
    Parser::parse(input)
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, SyntaxError> {
    Lexer::new(input).tokenize_all()
}
