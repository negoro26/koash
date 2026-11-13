use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::sink::Sink;

/// What a command consumes from the previous stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    None,
    Lines,
    Table,
    Bytes,
}

/// Where in a pipeline a command may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Any,
    /// Mutates shell state, so it runs only as the whole foreground pipeline.
    /// Anywhere else the word falls through to the external launcher.
    SoleStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxPolicy {
    Track,
    /// The pipeline does not open a ledger entry (undo, rollback, tx-log).
    Skip,
}

/// One option a command accepts. `args::Args::parse` matches arguments against these and
/// `help`, `discover`, the schemas endpoint, and completion render the same list.
#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub short: Option<char>,
    /// Empty when the flag has no long form.
    pub long: &'static str,
    /// Takes a value: `-n 5`, `-n5`, `--lines 5`, `--lines=5`.
    pub value: bool,
    pub help: &'static str,
    /// The long name is spelled with one dash (`find -name`) and never with two.
    pub single_dash: bool,
    /// `-5` is shorthand for this flag with the value `5` (`head -5`, `kill -9`).
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

    /// `-r` and `--recursive`.
    pub const fn switch(short: char, long: &'static str, help: &'static str) -> Self {
        Self::new(Some(short), long, false, help)
    }

    /// `--hidden`.
    pub const fn long_switch(long: &'static str, help: &'static str) -> Self {
        Self::new(None, long, false, help)
    }

    /// `-l`, for options that have no long form.
    pub const fn short_switch(short: char, help: &'static str) -> Self {
        Self::new(Some(short), "", false, help)
    }

    /// `-n N` and `--lines N`.
    pub const fn valued(short: char, long: &'static str, help: &'static str) -> Self {
        Self::new(Some(short), long, true, help)
    }

    /// `--limit N`.
    pub const fn long_valued(long: &'static str, help: &'static str) -> Self {
        Self::new(None, long, true, help)
    }

    /// `find -name PATTERN`.
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

    /// Whether `key` names this flag: its long name or its single-character short name.
    pub fn is(&self, key: &str) -> bool {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if self.short == Some(c) => true,
            _ => !self.long.is_empty() && self.long == key,
        }
    }

    /// How a user types this flag: `-r`, `--recursive`, or `-name`.
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

/// Every spelling of every flag, in declaration order.
pub fn flag_spellings(flags: &[Flag]) -> Vec<String> {
    flags.iter().flat_map(Flag::spellings).collect()
}

#[derive(Debug)]
pub struct Signature {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub flags: &'static [Flag],
    pub columns: &'static [&'static str],
    pub input: InputMode,
    pub placement: Placement,
    pub tx: TxPolicy,
}

pub trait Builtin: Sync {
    fn signature(&self) -> &'static Signature;

    /// Whether this invocation consumes upstream or stdin input.
    /// Commands that fall back to stdin only without file operands override it.
    fn reads_input(&self, _args: &[String]) -> bool {
        self.signature().input != InputMode::None
    }

    /// Entry point for `Placement::Any` commands.
    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic>;

    /// Entry point for `Placement::SoleStage` commands. Returns the exit status.
    fn run_alone(&self, _session: &mut Session, _args: &[String], _sink: &Sink) -> i32 {
        2
    }
}
