use shell_kernel::{History, Session};
use shell_value::{Rows, StructuredDiagnostic, UnixTime, Value, record};

use super::fs_ops::fail;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

record! {
    struct HistoryRow<'a> {
        n: u64,
        time: UnixTime,
        cwd: &'a str,
        exit: u64,
        #[text] command: &'a str,
    }
}

pub(super) struct HistoryList;

static SIGNATURE: Signature = Signature {
    name: "history",
    aliases: &[],
    description: "List the interactive command history, optionally only the last N entries",
    flags: &[],
    columns: crate::builtin::columns!["n", "time", "cwd", "exit", "command"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: false,
};

impl Builtin for HistoryList {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let shown = match args.operands.as_slice() {
            [] => None,
            [count] => match count.parse::<usize>() {
                Ok(count) => Some(count),
                Err(_) => {
                    fail(
                        session,
                        "E_HISTORY",
                        format!("history: {count}: numeric argument required"),
                    );
                    return Ok(Value::Empty);
                }
            },
            _ => {
                fail(session, "E_HISTORY", "history: too many arguments".into());
                return Ok(Value::Empty);
            }
        };
        let history = History::load_default();
        let entries = history.entries();
        let skip = shown.map_or(0, |count| entries.len().saturating_sub(count));
        let mut rows = Rows::<HistoryRow<'static>>::with_capacity(entries.len() - skip);
        for (index, entry) in entries.iter().enumerate().skip(skip) {
            rows.push(&HistoryRow {
                n: index as u64 + 1,
                time: UnixTime(entry.timestamp),
                cwd: &entry.cwd,
                exit: u64::from(entry.exit_code.unsigned_abs() & 0xff),
                command: &entry.command,
            });
        }
        Ok(Value::from_batch(rows.finish_or_empty()?))
    }
}
