use std::str::FromStr;

use shell_value::StructuredDiagnostic;

use crate::builtin::{Flag, Signature, flag_spellings};

pub struct Args<'a> {
    given: Vec<(&'static Flag, Option<&'a str>)>,
    pub operands: Vec<&'a str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gnu,
    Leading,
}

fn args_error(sig: &Signature, message: String) -> StructuredDiagnostic {
    let accepted = flag_spellings(sig.flags);
    let hint = if accepted.is_empty() {
        format!("{} takes no options", sig.name)
    } else {
        format!("{} accepts: {}", sig.name, accepted.join(" "))
    };
    StructuredDiagnostic::new(
        "E_ARGS",
        format!("{}: {message}", sig.name),
        None,
        Some(hint),
    )
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

enum Parsed<'a> {
    Flags(Vec<(&'static Flag, Option<&'a str>)>),
    Operand,
}

enum Rejected {
    UnknownOption(String),
    Invalid(StructuredDiagnostic),
}

impl Rejected {
    fn into_diagnostic(self, sig: &Signature) -> StructuredDiagnostic {
        match self {
            Self::UnknownOption(spelling) => {
                args_error(sig, format!("unknown option '{spelling}'"))
            }
            Self::Invalid(diag) => diag,
        }
    }
}

impl From<StructuredDiagnostic> for Rejected {
    fn from(diag: StructuredDiagnostic) -> Self {
        Self::Invalid(diag)
    }
}

impl<'a> Args<'a> {
    pub fn parse(
        sig: &'static Signature,
        args: &'a [String],
    ) -> Result<Self, StructuredDiagnostic> {
        Self::parse_with(sig, args, Mode::Gnu).map_err(|rejected| rejected.into_diagnostic(sig))
    }

    pub fn unknown_option(sig: &'static Signature, args: &'a [String]) -> Option<String> {
        if !args.iter().any(|a| a.len() > 1 && a.starts_with('-')) {
            return None;
        }
        match Self::parse_with(sig, args, Mode::Gnu) {
            Err(Rejected::UnknownOption(spelling)) => Some(spelling),
            _ => None,
        }
    }

    pub fn parse_leading(
        sig: &'static Signature,
        args: &'a [String],
    ) -> Result<Self, StructuredDiagnostic> {
        Self::parse_with(sig, args, Mode::Leading).map_err(|rejected| rejected.into_diagnostic(sig))
    }

    fn parse_with(
        sig: &'static Signature,
        args: &'a [String],
        mode: Mode,
    ) -> Result<Self, Rejected> {
        let mut out = Args {
            given: Vec::new(),
            operands: Vec::new(),
        };
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            i += 1;
            if arg == "--" && mode == Mode::Gnu {
                out.operands.extend(args[i..].iter().map(String::as_str));
                break;
            }
            let parsed = if arg.len() > 1 && arg.starts_with('-') {
                parse_one(sig, arg, args, &mut i)
            } else {
                Ok(Parsed::Operand)
            };
            match (parsed, mode) {
                (Ok(Parsed::Flags(found)), _) => out.given.extend(found),
                (Ok(Parsed::Operand), Mode::Gnu) => out.operands.push(arg),
                (Err(err), Mode::Gnu) => return Err(err),
                (Ok(Parsed::Operand) | Err(_), Mode::Leading) => {
                    out.operands
                        .extend(args[i - 1..].iter().map(String::as_str));
                    break;
                }
            }
        }
        Ok(out)
    }

    pub fn has(&self, key: &str) -> bool {
        self.given.iter().any(|(flag, _)| flag.is(key))
    }

    pub fn value(&self, key: &str) -> Option<&'a str> {
        self.values(key).last()
    }

    pub fn values(&self, key: &str) -> impl Iterator<Item = &'a str> {
        self.given
            .iter()
            .filter(move |(flag, _)| flag.is(key))
            .filter_map(|(_, value)| *value)
    }

    pub fn last_of<'k>(&self, keys: &[&'k str]) -> Option<&'k str> {
        self.given
            .iter()
            .rev()
            .find_map(|(flag, _)| keys.iter().copied().find(|k| flag.is(k)))
    }

    pub fn number<T: FromStr>(
        &self,
        sig: &Signature,
        key: &str,
    ) -> Result<Option<T>, StructuredDiagnostic> {
        self.value(key)
            .map(|raw| {
                raw.parse::<T>()
                    .map_err(|_| args_error(sig, format!("invalid value '{raw}' for '{key}'")))
            })
            .transpose()
    }
}

fn find_long(sig: &'static Signature, name: &str, single_dash: bool) -> Option<&'static Flag> {
    sig.flags
        .iter()
        .find(|f| !f.long.is_empty() && f.long == name && f.single_dash == single_dash)
}

fn parse_one<'a>(
    sig: &'static Signature,
    arg: &'a str,
    args: &'a [String],
    next: &mut usize,
) -> Result<Parsed<'a>, Rejected> {
    if let Some(long) = arg.strip_prefix("--") {
        let (name, inline) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (long, None),
        };
        let flag = find_long(sig, name, false)
            .ok_or_else(|| Rejected::UnknownOption(format!("--{name}")))?;
        let value = match (flag.value, inline) {
            (true, Some(v)) => Some(v),
            (true, None) => Some(take_value(sig, flag, args, next)?),
            (false, None) => None,
            (false, Some(_)) => {
                return Err(
                    args_error(sig, format!("option '--{name}' does not take a value")).into(),
                );
            }
        };
        return Ok(Parsed::Flags(vec![(flag, value)]));
    }

    let body = &arg[1..];
    if let Some(flag) = find_long(sig, body, true) {
        let value = if flag.value {
            Some(take_value(sig, flag, args, next)?)
        } else {
            None
        };
        return Ok(Parsed::Flags(vec![(flag, value)]));
    }
    if is_number(body)
        && let Some(flag) = sig.flags.iter().find(|f| f.numeric)
    {
        return Ok(Parsed::Flags(vec![(flag, Some(body))]));
    }

    let mut found = Vec::new();
    for (at, c) in body.char_indices() {
        let flag = sig
            .flags
            .iter()
            .find(|f| f.short == Some(c))
            .ok_or_else(|| Rejected::UnknownOption(format!("-{c}")))?;
        if !flag.value {
            found.push((flag, None));
            continue;
        }
        let attached = &body[at + c.len_utf8()..];
        let value = if attached.is_empty() {
            take_value(sig, flag, args, next)?
        } else {
            attached
        };
        found.push((flag, Some(value)));
        break;
    }
    Ok(Parsed::Flags(found))
}

fn take_value<'a>(
    sig: &'static Signature,
    flag: &Flag,
    args: &'a [String],
    next: &mut usize,
) -> Result<&'a str, StructuredDiagnostic> {
    let value = args.get(*next).ok_or_else(|| {
        let spelling = flag.spellings().last().unwrap_or_default();
        args_error(sig, format!("option '{spelling}' requires a value"))
    })?;
    *next += 1;
    Ok(value.as_str())
}
