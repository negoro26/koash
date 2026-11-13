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

#[derive(Debug)]
pub struct Signature {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub flags: &'static [&'static str],
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
