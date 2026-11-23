use shell_kernel::Session;
use shell_value::{Record, Rows, StructuredDiagnostic, Value, record};

use super::common::usage_error;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Set;

record! {
    struct OptionRow<'a> {
        #[text] option: &'a str,
        enabled: bool,
    }
}

static SIGNATURE: Signature = Signature {
    name: "set",
    aliases: &[],
    description: "Inspect or toggle shell options (-e/+e errexit, -f/+f noglob, -o/+o pipefail)",
    flags: &[],
    columns: <OptionRow<'static> as Record>::COLUMNS,
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
    strict_options: false,
};

impl Builtin for Set {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        if args.is_empty() || (args.len() == 1 && args[0] == "-o") {
            let mut rows = Rows::<OptionRow<'static>>::with_capacity(3);
            rows.push(&OptionRow {
                option: "errexit",
                enabled: session.errexit,
            });
            rows.push(&OptionRow {
                option: "noglob",
                enabled: session.noglob,
            });
            rows.push(&OptionRow {
                option: "pipefail",
                enabled: session.pipefail,
            });
            return Ok(Value::from_batch(rows.finish()?));
        }

        const USAGE: &str = "set [-e|+e] [-f|+f] [-o|+o errexit|noglob|pipefail]";
        let mut rest = args.iter().map(String::as_str);
        while let Some(arg) = rest.next() {
            let enable = match arg.chars().next() {
                Some('-') => true,
                Some('+') => false,
                _ => return Err(usage_error(format!("set: unsupported flag `{arg}`"), USAGE)),
            };
            if arg.len() == 1 {
                return Err(usage_error(format!("set: unsupported flag `{arg}`"), USAGE));
            }
            for letter in arg[1..].chars() {
                match letter {
                    'e' => session.errexit = enable,
                    'f' => session.noglob = enable,
                    'o' => match rest.next() {
                        Some("errexit") => session.errexit = enable,
                        Some("noglob") => session.noglob = enable,
                        Some("pipefail") => session.pipefail = enable,
                        Some(other) => {
                            return Err(usage_error(
                                format!("set: unknown option `{other}`"),
                                USAGE,
                            ));
                        }
                        None => {
                            return Err(usage_error("set: -o/+o requires an option name", USAGE));
                        }
                    },
                    _ => {
                        return Err(usage_error(format!("set: unsupported flag `{arg}`"), USAGE));
                    }
                }
            }
        }
        Ok(Value::Empty)
    }
}
