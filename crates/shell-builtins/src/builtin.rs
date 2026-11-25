use shell_kernel::Session;
use shell_value::{Column, StructuredDiagnostic, Value};

use crate::sink::Sink;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    None,
    Lines,
    Table,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Any,
    SoleStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxPolicy {
    Track,
    Skip,
}

#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub short: Option<char>,
    pub long: &'static str,
    pub value: bool,
    pub help: &'static str,
    pub single_dash: bool,
    pub numeric: bool,
}

impl Flag {
    const fn new(short: Option<char>, long: &'static str, value: bool, help: &'static str) -> Self {
        Self {
            short,
            long,
            value,
            help,
            single_dash: false,
            numeric: false,
        }
    }

    pub const fn switch(short: char, long: &'static str, help: &'static str) -> Self {
        Self::new(Some(short), long, false, help)
    }

    pub const fn long_switch(long: &'static str, help: &'static str) -> Self {
        Self::new(None, long, false, help)
    }

    pub const fn short_switch(short: char, help: &'static str) -> Self {
        Self::new(Some(short), "", false, help)
    }

    pub const fn valued(short: char, long: &'static str, help: &'static str) -> Self {
        Self::new(Some(short), long, true, help)
    }

    pub const fn long_valued(long: &'static str, help: &'static str) -> Self {
        Self::new(None, long, true, help)
    }

    pub const fn predicate(long: &'static str, help: &'static str) -> Self {
        let mut flag = Self::new(None, long, true, help);
        flag.single_dash = true;
        flag
    }

    pub const fn with_short(mut self, short: char) -> Self {
        self.short = Some(short);
        self
    }

    pub const fn numeric(mut self) -> Self {
        self.numeric = true;
        self
    }

    pub fn is(&self, key: &str) -> bool {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if self.short == Some(c) => true,
            _ => !self.long.is_empty() && self.long == key,
        }
    }

    pub fn spellings(&self) -> impl Iterator<Item = String> {
        let short = self.short.map(|c| format!("-{c}"));
        let long = (!self.long.is_empty()).then(|| {
            if self.single_dash {
                format!("-{}", self.long)
            } else {
                format!("--{}", self.long)
            }
        });
        short.into_iter().chain(long)
    }
}

pub fn flag_spellings(flags: &[Flag]) -> Vec<String> {
    flags.iter().flat_map(Flag::spellings).collect()
}

macro_rules! columns {
    ($($name:literal),* $(,)?) => {
        &[$(::shell_value::Column::new($name)),*]
    };
}
pub(crate) use columns;

#[derive(Debug)]
pub struct Signature {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub flags: &'static [Flag],
    pub columns: &'static [Column],
    pub input: InputMode,
    pub placement: Placement,
    pub tx: TxPolicy,
    pub strict_options: bool,
}

impl Signature {
    pub fn column_names(&self) -> impl Iterator<Item = &'static str> {
        self.columns.iter().map(Column::name)
    }
}

pub trait Builtin: Sync {
    fn signature(&self) -> &'static Signature;

    fn reads_input(&self, _args: &[String]) -> bool {
        self.signature().input != InputMode::None
    }

    fn text_writes(&self, _args: &[String]) -> Option<Vec<String>> {
        None
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic>;

    fn run_alone(&self, _session: &mut Session, _args: &[String], _sink: &Sink) -> i32 {
        2
    }
}
