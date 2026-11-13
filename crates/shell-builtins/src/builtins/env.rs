use std::path::PathBuf;

use serde_json::Value as JsonVal;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::resolve_path;

pub(crate) fn cd(session: &mut Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
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

pub(crate) fn pwd(session: &Session) -> Result<Value, StructuredDiagnostic> {
    let mut rec = StructuredRecord::new();
    rec.insert(
        "path".to_string(),
        JsonVal::String(session.cwd.to_string_lossy().to_string()),
    );
    Ok(Value::from_records(vec![rec]))
}

pub(crate) fn export(
    session: &mut Session,
    args: &[String],
) -> Result<Value, StructuredDiagnostic> {
    for arg in args {
        if let Some((k, v)) = arg.split_once('=') {
            session.env.insert(k.to_string(), v.to_string());
        }
    }
    Ok(Value::Empty)
}

pub(crate) fn env(session: &Session) -> Result<Value, StructuredDiagnostic> {
    let records = session
        .env
        .materialize()
        .into_iter()
        .map(|(k, v)| {
            let mut rec = StructuredRecord::new();
            rec.insert("key".to_string(), JsonVal::String(k));
            rec.insert("value".to_string(), JsonVal::String(v));
            rec
        })
        .collect();
    Ok(Value::from_records(records))
}
