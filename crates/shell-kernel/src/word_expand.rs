use shell_syntax::{Word, WordPart};

use crate::env::SessionEnv;

pub fn expand_word(word: &Word, env: &SessionEnv, last_status: i32) -> String {
    let mut out = String::new();
    for part in &word.parts {
        expand_part(part, env, last_status, &mut out);
    }
    out
}

fn expand_part(
    part: &WordPart,
    env: &SessionEnv,
    last_status: i32,
    out: &mut String,
) {
    match part {
        WordPart::Literal { text, .. } | WordPart::SingleQuoted { text, .. } => {
            out.push_str(text);
        }
        WordPart::DoubleQuoted { parts, .. } => {
            for p in parts {
                expand_part(p, env, last_status, out);
            }
        }
        WordPart::Var { name, .. } => match name.as_str() {
            "?" => out.push_str(&last_status.to_string()),
            "$" => out.push_str(&std::process::id().to_string()),
            _ => {
                if let Some(val) = env.get(name) {
                    out.push_str(&val);
                }
            }
        },
        WordPart::Tilde { .. } => {
            if let Some(home) = env.get("HOME") {
                out.push_str(&home);
            } else {
                out.push('~');
            }
        }
    }
}
