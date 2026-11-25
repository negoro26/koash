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
    description: "Inspect or toggle shell options (-e errexit, -f noglob, -u nounset, -o pipefail)",
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
            let mut rows = Rows::<OptionRow<'static>>::with_capacity(4);
            rows.push(&OptionRow {
                option: "errexit",
                enabled: session.errexit,
            });
            rows.push(&OptionRow {
                option: "noglob",
                enabled: session.noglob,
            });
            rows.push(&OptionRow {
                option: "nounset",
                enabled: session.nounset,
            });
            rows.push(&OptionRow {
                option: "pipefail",
                enabled: session.pipefail,
            });
            return Ok(Value::from_batch(rows.finish()?));
        }

        let (changes, deferred) = parse(args)?;
        let mut ignored = Vec::new();
        for (option, enable) in changes {
            match option.support {
                Support::Errexit => session.errexit = enable,
                Support::Noglob => session.noglob = enable,
                Support::Nounset => session.nounset = enable,
                Support::Pipefail => session.pipefail = enable,
                Support::Ignored => ignored.push(option.name),
                Support::Refused => {}
            }
        }
        if !ignored.is_empty() {
            session.report(StructuredDiagnostic::new(
                "W_UNSUPPORTED",
                format!("set: {} not implemented, ignored", ignored.join(", ")),
                None,
                None,
            ));
        }
        match deferred {
            Some(diag) => Err(diag),
            None => Ok(Value::Empty),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Support {
    Errexit,
    Noglob,
    Nounset,
    Pipefail,
    Ignored,
    Refused,
}

struct ShellOption {
    name: &'static str,
    letter: Option<char>,
    support: Support,
}

const fn opt(name: &'static str, letter: Option<char>, support: Support) -> ShellOption {
    ShellOption {
        name,
        letter,
        support,
    }
}

static OPTIONS: [ShellOption; 27] = [
    opt("allexport", Some('a'), Support::Ignored),
    opt("braceexpand", Some('B'), Support::Ignored),
    opt("emacs", None, Support::Ignored),
    opt("errexit", Some('e'), Support::Errexit),
    opt("errtrace", Some('E'), Support::Ignored),
    opt("functrace", Some('T'), Support::Ignored),
    opt("hashall", Some('h'), Support::Ignored),
    opt("histexpand", Some('H'), Support::Ignored),
    opt("history", None, Support::Ignored),
    opt("ignoreeof", None, Support::Ignored),
    opt("interactive-comments", None, Support::Ignored),
    opt("keyword", Some('k'), Support::Ignored),
    opt("monitor", Some('m'), Support::Ignored),
    opt("noclobber", Some('C'), Support::Ignored),
    opt("noexec", Some('n'), Support::Refused),
    opt("noglob", Some('f'), Support::Noglob),
    opt("nolog", None, Support::Ignored),
    opt("notify", Some('b'), Support::Ignored),
    opt("nounset", Some('u'), Support::Nounset),
    opt("onecmd", Some('t'), Support::Refused),
    opt("physical", Some('P'), Support::Ignored),
    opt("pipefail", None, Support::Pipefail),
    opt("posix", None, Support::Ignored),
    opt("privileged", Some('p'), Support::Ignored),
    opt("verbose", Some('v'), Support::Ignored),
    opt("vi", None, Support::Ignored),
    opt("xtrace", Some('x'), Support::Ignored),
];

const USAGE: &str = "set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]";

type Changes = Vec<(&'static ShellOption, bool)>;

fn parse(args: &[String]) -> Result<(Changes, Option<StructuredDiagnostic>), StructuredDiagnostic> {
    let mut changes = Vec::new();
    let mut rest = args.iter().map(String::as_str);
    while let Some(arg) = rest.next() {
        let enable = match arg.chars().next() {
            Some('-') => true,
            Some('+') => false,
            _ => return Err(positional_error()),
        };
        match &arg[1..] {
            "" => break,
            "-" if enable => {
                if rest.next().is_some() {
                    return Err(positional_error());
                }
                break;
            }
            letters => {
                for letter in letters.chars() {
                    let option = if letter == 'o' {
                        let Some(name) = rest.next() else {
                            let diag = usage_error("set: -o/+o requires an option name", USAGE);
                            return Ok((changes, Some(diag)));
                        };
                        let Some(option) = OPTIONS.iter().find(|o| o.name == name) else {
                            let diag =
                                usage_error(format!("set: {name}: invalid option name"), USAGE);
                            return Ok((changes, Some(diag)));
                        };
                        option
                    } else {
                        OPTIONS
                            .iter()
                            .find(|o| o.letter == Some(letter))
                            .ok_or_else(|| {
                                let sign = if enable { '-' } else { '+' };
                                usage_error(format!("set: {sign}{letter}: invalid option"), USAGE)
                            })?
                    };
                    if option.support == Support::Refused {
                        return Err(usage_error(
                            format!("set: {} is not supported", option.name),
                            USAGE,
                        ));
                    }
                    changes.push((option, enable));
                }
            }
        }
    }
    Ok((changes, None))
}

fn positional_error() -> StructuredDiagnostic {
    usage_error("set: positional parameters are not supported", USAGE)
}
