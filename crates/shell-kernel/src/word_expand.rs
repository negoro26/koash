use shell_syntax::{Word, WordPart};

use crate::brace;
use crate::env::SessionEnv;
use crate::pathname;
use crate::session::Session;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Raw,
    Value,
    Quoted,
}

#[derive(Clone, Copy)]
pub(crate) struct Sym {
    pub ch: char,
    pub kind: Kind,
}

struct Field {
    text: String,
    runs: Vec<(usize, Kind)>,
    keep_empty: bool,
}

impl Field {
    fn new(word: &Word, env: &SessionEnv, last_status: i32) -> Self {
        let mut field = Field {
            text: String::new(),
            runs: Vec::new(),
            keep_empty: word.parts.iter().any(kept_when_empty),
        };
        for part in &word.parts {
            field.push_part(part, false, env, last_status);
        }
        field
    }

    fn push(&mut self, s: &str, kind: Kind) {
        if s.is_empty() {
            return;
        }
        self.text.push_str(s);
        let end = self.text.len();
        match self.runs.last_mut() {
            Some((run_end, run_kind)) if *run_kind == kind => *run_end = end,
            _ => self.runs.push((end, kind)),
        }
    }

    fn push_part(&mut self, part: &WordPart, quoted: bool, env: &SessionEnv, last_status: i32) {
        let unquoted = |kind| if quoted { Kind::Quoted } else { kind };
        match part {
            WordPart::Literal { text, .. } => self.push(text, unquoted(Kind::Raw)),
            WordPart::SingleQuoted { text, .. } => self.push(text, Kind::Quoted),
            WordPart::Escaped { ch, .. } => self.push(ch.encode_utf8(&mut [0; 4]), Kind::Quoted),
            WordPart::DoubleQuoted { parts, .. } => {
                for p in parts {
                    self.push_part(p, true, env, last_status);
                }
            }
            WordPart::Var { name, .. } => {
                let kind = unquoted(Kind::Value);
                match name.as_str() {
                    "?" => self.push(&last_status.to_string(), kind),
                    "$" => self.push(&std::process::id().to_string(), kind),
                    _ => {
                        if let Some(val) = env.get(name) {
                            self.push(&val, kind);
                        }
                    }
                }
            }
            WordPart::Tilde { .. } => match env.get("HOME") {
                Some(home) => self.push(&home, Kind::Quoted),
                None => self.push("~", Kind::Quoted),
            },
        }
    }

    fn needs_expansion(&self) -> bool {
        let mut start = 0;
        self.runs.iter().any(|&(end, kind)| {
            let run = &self.text[start..end];
            start = end;
            match kind {
                Kind::Raw => run.contains(['{', '*', '?', '[']),
                Kind::Value => run.contains(['*', '?', '[']),
                Kind::Quoted => false,
            }
        })
    }

    fn syms(&self) -> Vec<Sym> {
        let mut start = 0;
        let mut syms = Vec::with_capacity(self.text.len());
        for &(end, kind) in &self.runs {
            syms.extend(self.text[start..end].chars().map(|ch| Sym { ch, kind }));
            start = end;
        }
        syms
    }
}

pub fn expand_word(word: &Word, env: &SessionEnv, last_status: i32) -> String {
    Field::new(word, env, last_status).text
}

pub fn expand_words(words: &[Word], session: &Session) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    for word in words {
        let field = Field::new(word, &session.env, session.last_status);
        if !field.needs_expansion() {
            if !field.text.is_empty() || field.keep_empty {
                out.push(field.text);
            }
            continue;
        }
        for syms in brace::expand(&field.syms()) {
            push_pathname(&syms, field.keep_empty, session, &mut out);
        }
    }
    out
}

pub fn command_word(words: &[Word], session: &Session) -> Option<String> {
    words.iter().find_map(|word| {
        let field = Field::new(word, &session.env, session.last_status);
        let first = if field.needs_expansion() {
            let syms = brace::expand(&field.syms()).into_iter().next()?;
            syms.iter().map(|s| s.ch).collect()
        } else {
            field.text
        };
        (!first.is_empty() || field.keep_empty).then_some(first)
    })
}

fn push_pathname(syms: &[Sym], keep_empty: bool, session: &Session, out: &mut Vec<String>) {
    if pathname::has_meta(syms) {
        let matches = pathname::expand(syms, &session.cwd);
        if !matches.is_empty() {
            out.extend(matches);
            return;
        }
    }
    let text: String = syms.iter().map(|s| s.ch).collect();
    if !text.is_empty() || keep_empty {
        out.push(text);
    }
}

fn kept_when_empty(part: &WordPart) -> bool {
    matches!(
        part,
        WordPart::SingleQuoted { .. } | WordPart::DoubleQuoted { .. } | WordPart::Tilde { .. }
    )
}
