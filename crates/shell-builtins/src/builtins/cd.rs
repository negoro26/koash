use std::path::PathBuf;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Cd;

static SIGNATURE: Signature = Signature {
    name: "cd",
    aliases: &[],
    description: "Change working directory",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Cd {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let arg = match args.first().map(|s| s.as_str()) {
            Some("--") => args.get(1).map(|s| s.as_str()),
            other => other,
        };
        let target = match arg {
            Some("-") => match session.env.get("OLDPWD") {
                Some(old) => PathBuf::from(old),
                None => {
                    return Err(StructuredDiagnostic::new(
                        "E_CD",
                        "OLDPWD not set",
                        None,
                        None,
                    ));
                }
            },
            Some(path) => resolve_path(session, path),
            None => {
                let home = session.env.get("HOME").unwrap_or_else(|| "/".to_string());
                PathBuf::from(home)
            }
        };

        let canonical = std::fs::canonicalize(&target)
            .map_err(|e| StructuredDiagnostic::from_io("E_CD", &e, None))?;
        if !canonical.is_dir() {
            return Err(StructuredDiagnostic::new(
                "E_CD",
                format!("{}: not a directory", canonical.display()),
                None,
                None,
            ));
        }
        let old = session.cwd.to_string_lossy().to_string();
        let s = canonical.to_string_lossy().to_string();
        session.env.insert("OLDPWD".to_string(), old);
        session.cwd = canonical;
        session.env.insert("PWD".to_string(), s);
        Ok(Value::Empty)
    }
}
