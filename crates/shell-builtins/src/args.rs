//! The one argument parser. Every builtin that takes options declares them as
//! `Flag`s in its `Signature` and reads the result from `Args`.
//!
//! Syntax follows the POSIX utility guidelines plus GNU long options, as clap does
//! for uutils: short clusters (`-rf`), `-n5` and `-n 5`, exact long names
//! (`--force`, never a prefix of one), `--long=value`, `--` ends options, and a lone
//! `-` is an operand. Options and operands may be interleaved. An unknown option or
//! a missing value is an `E_ARGS` error, so nothing runs on a mistyped flag.

use std::str::FromStr;

use shell_value::StructuredDiagnostic;

use crate::builtin::{Flag, Signature, flag_spellings};

pub struct Args<'a> {
    given: Vec<(&'static Flag, Option<&'a str>)>,
    pub operands: Vec<&'a str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Options anywhere, `--` terminates them.
    Gnu,
    /// POSIX `echo`: options only before the first operand, and a dash word that is not
    /// entirely made of known flags is an operand, `--` included.
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

impl<'a> Args<'a> {
    pub fn parse(
        sig: &'static Signature,
        args: &'a [String],
    ) -> Result<Self, StructuredDiagnostic> {
        Self::parse_with(sig, args, Mode::Gnu)
    }

    /// `echo`-style parsing: see `Mode::Leading`.
    pub fn parse_leading(
        sig: &'static Signature,
        args: &'a [String],
    ) -> Result<Self, StructuredDiagnostic> {
        Self::parse_with(sig, args, Mode::Leading)
    }

    fn parse_with(
        sig: &'static Signature,
        args: &'a [String],
        mode: Mode,
    ) -> Result<Self, StructuredDiagnostic> {
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

    /// The value of the last occurrence of a valued flag.
    pub fn value(&self, key: &str) -> Option<&'a str> {
        self.values(key).last()
    }

    /// Every value of a repeatable valued flag, in command-line order.
    pub fn values(&self, key: &str) -> impl Iterator<Item = &'a str> {
        self.given
            .iter()
            .filter(move |(flag, _)| flag.is(key))
            .filter_map(|(_, value)| *value)
    }

    /// Which of `keys` was given last. Models flags that override each other (`-L`/`-P`, `-n`/`-f`).
    pub fn last_of<'k>(&self, keys: &[&'k str]) -> Option<&'k str> {
        self.given
            .iter()
            .rev()
            .find_map(|(flag, _)| keys.iter().copied().find(|k| flag.is(k)))
    }

    /// The last value of `key` parsed as `T`; an unparsable value is a usage error.
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
) -> Result<Parsed<'a>, StructuredDiagnostic> {
    if let Some(long) = arg.strip_prefix("--") {
        let (name, inline) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (long, None),
        };
        let flag = find_long(sig, name, false)
            .ok_or_else(|| args_error(sig, format!("unknown option '--{name}'")))?;
        let value = match (flag.value, inline) {
            (true, Some(v)) => Some(v),
            (true, None) => Some(take_value(sig, flag, args, next)?),
            (false, None) => None,
            (false, Some(_)) => {
                return Err(args_error(
                    sig,
                    format!("option '--{name}' does not take a value"),
                ));
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
            .ok_or_else(|| args_error(sig, format!("unknown option '-{c}'")))?;
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

#[cfg(test)]
mod tests {
    use crate::builtin::{Flag, InputMode, Placement, Signature, TxPolicy};

    use super::*;

    static SIG: Signature = Signature {
        name: "t",
        aliases: &[],
        description: "",
        flags: &[
            Flag::switch('r', "recursive", ""),
            Flag::short_switch('R', ""),
            Flag::switch('f', "force", ""),
            Flag::valued('n', "lines", "").numeric(),
            Flag::long_valued("include", ""),
            Flag::predicate("name", ""),
            Flag::long_switch("hidden", ""),
        ],
        columns: &[],
        input: InputMode::None,
        placement: Placement::Any,
        tx: TxPolicy::Track,
    };

    static PLAIN: Signature = Signature {
        name: "p",
        aliases: &[],
        description: "",
        flags: &[],
        columns: &[],
        input: InputMode::None,
        placement: Placement::Any,
        tx: TxPolicy::Track,
    };

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    fn flags_of(args: &Args) -> Vec<String> {
        args.given
            .iter()
            .map(|(f, v)| match v {
                Some(v) => format!("{}={v}", f.long),
                None => f.long.to_string(),
            })
            .collect()
    }

    fn message(words: &[&str]) -> String {
        let owned = argv(words);
        Args::parse(&SIG, &owned)
            .err()
            .map(|e| e.message)
            .unwrap_or_default()
    }

    #[test]
    fn short_clusters_and_exact_long_names() {
        let owned = argv(&["-rf", "a", "--hidden", "b"]);
        let args = Args::parse(&SIG, &owned).unwrap();
        assert_eq!(flags_of(&args), ["recursive", "force", "hidden"]);
        assert_eq!(args.operands, ["a", "b"]);
    }

    #[test]
    fn a_long_name_never_matches_by_substring_or_prefix() {
        assert!(message(&["--forc"]).contains("unknown option '--forc'"));
        assert!(message(&["--recursive-ish"]).contains("--recursive-ish"));
        assert!(message(&["--verbose"]).contains("--verbose"));
    }

    #[test]
    fn value_flags_take_attached_separate_and_equals_forms() {
        for words in [
            &["-n5"][..],
            &["-n", "5"],
            &["--lines", "5"],
            &["--lines=5"],
            &["-rn5"],
            &["-rn", "5"],
        ] {
            let owned = argv(words);
            let args = Args::parse(&SIG, &owned).unwrap();
            assert_eq!(args.value("n"), Some("5"), "{words:?}");
            assert!(args.operands.is_empty(), "{words:?}");
        }
    }

    #[test]
    fn a_missing_value_is_an_error_naming_the_flag() {
        assert!(message(&["-n"]).contains("'--lines' requires a value"));
        assert!(message(&["--include"]).contains("requires a value"));
    }

    #[test]
    fn a_value_for_a_switch_is_an_error() {
        assert!(message(&["--force=1"]).contains("does not take a value"));
    }

    #[test]
    fn double_dash_ends_options_and_a_lone_dash_is_an_operand() {
        let owned = argv(&["-r", "-", "--", "-f", "--hidden"]);
        let args = Args::parse(&SIG, &owned).unwrap();
        assert_eq!(flags_of(&args), ["recursive"]);
        assert_eq!(args.operands, ["-", "-f", "--hidden"]);
    }

    #[test]
    fn a_numeric_word_is_shorthand_for_the_numeric_flag() {
        let owned = argv(&["-12", "x"]);
        let args = Args::parse(&SIG, &owned).unwrap();
        assert_eq!(args.value("lines"), Some("12"));
        assert_eq!(args.operands, ["x"]);
    }

    #[test]
    fn a_number_is_an_error_when_no_flag_is_numeric() {
        let owned = argv(&["-5"]);
        let err = Args::parse(&PLAIN, &owned).err().map(|e| e.message);
        assert_eq!(err.as_deref(), Some("p: unknown option '-5'"));
    }

    #[test]
    fn single_dash_predicates_match_whole_words_only() {
        let owned = argv(&["-name", "*.rs"]);
        let args = Args::parse(&SIG, &owned).unwrap();
        assert_eq!(args.value("name"), Some("*.rs"));
        assert!(message(&["--name", "x"]).contains("unknown option '--name'"));
    }

    #[test]
    fn unknown_options_are_args_errors_with_a_hint() {
        let owned = argv(&["-x"]);
        let err = Args::parse(&SIG, &owned).err().unwrap();
        assert_eq!(err.code, "E_ARGS");
        assert!(err.hint.is_some_and(|h| h.contains("--recursive")));
    }

    #[test]
    fn last_of_reports_the_option_given_last() {
        let owned = argv(&["-r", "-f", "-r"]);
        let args = Args::parse(&SIG, &owned).unwrap();
        assert_eq!(args.last_of(&["force", "recursive"]), Some("recursive"));
        assert_eq!(args.last_of(&["hidden"]), None);
        assert!(args.has("r") && args.has("force") && !args.has("hidden"));
    }

    #[test]
    fn echo_style_stops_at_the_first_operand_or_unknown_word() {
        let owned = argv(&["-r", "-f", "hi", "-r"]);
        let args = Args::parse_leading(&SIG, &owned).unwrap();
        assert!(args.has("r") && args.has("f"));
        assert_eq!(args.operands, ["hi", "-r"]);

        let owned = argv(&["-rx", "--", "y"]);
        let args = Args::parse_leading(&SIG, &owned).unwrap();
        assert!(!args.has("r"));
        assert_eq!(args.operands, ["-rx", "--", "y"]);
    }

    #[test]
    fn number_parses_a_value_and_rejects_garbage() {
        let owned = argv(&["-n", "7"]);
        let args = Args::parse(&SIG, &owned).unwrap();
        assert_eq!(args.number::<usize>(&SIG, "n").unwrap(), Some(7));
        let owned = argv(&["-n", "x"]);
        let args = Args::parse(&SIG, &owned).unwrap();
        assert!(args.number::<usize>(&SIG, "n").is_err());
    }
}
