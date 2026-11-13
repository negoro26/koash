mod env;
mod fs;
mod process;
mod table;
mod text;
mod tx;
mod workspace;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::discover::run_discover;

pub fn is_async_builtin(name: &str) -> bool {
    matches!(name, "@py" | "@js")
}

pub fn run_builtin_sync(
    session: &mut Session,
    name: &str,
    args: &[String],
    input: Value,
) -> Result<Value, StructuredDiagnostic> {
    match name {
        "cd" => env::cd(session, args),
        "pwd" => env::pwd(session),
        "export" => env::export(session, args),
        "env" => env::env(session),
        "ls" => fs::ls(session, args),
        "stat" => fs::stat(session, args),
        "find" => fs::find(session, args),
        "mkdir" => fs::mkdir(session, args),
        "rm" => fs::rm(session, args),
        "cp" => fs::cp(session, args),
        "mv" => fs::mv(session, args),
        "echo" => text::echo(args),
        "cat" => text::cat(session, args, input),
        "head" => text::head(session, args, input),
        "tail" => text::tail(session, args, input),
        "grep" => text::grep(session, args, input),
        "patch" => text::patch(session, args),
        "where" => table::where_filter(args, input),
        "select" => table::select(args, input),
        "sort-by" | "sort_by" => table::sort_by(args, input),
        "first" => table::first(args, input),
        "last" => table::last(args, input),
        "from" => table::from(args, input),
        "to" => table::to(args, input),
        "ps" => process::ps(),
        "kill" => process::kill(args),
        "which" => process::which(session, args),
        "undo" => tx::undo(session),
        "rollback" => tx::rollback(session, args),
        "tx-log" | "tx_log" => tx::tx_log(session),
        "diag" => workspace::diag(session),
        "refs" => workspace::refs(session, args),
        "rename" => workspace::rename(session, args),
        "prewalk" => crate::code_intel::run_prewalk(session, args),
        "ast" => crate::code_intel::run_ast(session, args),
        "discover" => run_discover(args),
        "help" => help(args),
        "@sql" => crate::subkernel::run_sql_inprocess(session, &args.join(" ")),
        _ => Err(StructuredDiagnostic::new(
            "E_UNKNOWN_BUILTIN",
            format!("unknown builtin `{name}`"),
            None,
            None,
        )),
    }
}

pub async fn run_builtin(
    session: &mut Session,
    name: &str,
    args: &[String],
    input: Value,
) -> Result<Value, StructuredDiagnostic> {
    match name {
        "@py" => {
            session.mark_opaque("@py".to_string());
            crate::subkernel::run_persistent_worker(
                session,
                crate::subkernel::KernelKind::Python,
                &args.join(" "),
            )
            .await
        }
        "@js" => {
            session.mark_opaque("@js".to_string());
            crate::subkernel::run_persistent_worker(
                session,
                crate::subkernel::KernelKind::JavaScript,
                &args.join(" "),
            )
            .await
        }
        _ => {
            let mat = match input {
                Value::RawStream(_) => Value::Bytes(input.into_raw_bytes().await?),
                Value::RecordStream(_) => Value::Batch(input.into_batch().await?),
                other => other,
            };
            run_builtin_sync(session, name, args, mat)
        }
    }
}

pub(crate) fn resolve_path(session: &Session, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        session.cwd.join(p)
    }
}

pub(crate) fn hashline_schema() -> Arc<[Cow<'static, str>]> {
    Arc::new([
        Cow::Borrowed("file"),
        Cow::Borrowed("line"),
        Cow::Borrowed("hash"),
        Cow::Borrowed("text"),
    ])
}

fn help(args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let cols: Arc<[Cow<'static, str>]> = Arc::new([
        Cow::Borrowed("command"),
        Cow::Borrowed("flags"),
        Cow::Borrowed("columns"),
        Cow::Borrowed("description"),
    ]);
    let filter = args.first().map(|s| s.as_str());
    let mut batch = shell_value::RecordBatch::with_capacity(
        cols,
        crate::schema::BUILTIN_SCHEMAS.len(),
    );
    for schema in crate::schema::BUILTIN_SCHEMAS {
        if let Some(q) = filter
            && !schema.name.eq_ignore_ascii_case(q)
            && !schema.name.contains(q)
            && !schema.description.contains(q)
        {
            continue;
        }
        batch.cells.push(shell_value::CellVal::Str(schema.name.to_string()));
        batch.cells.push(shell_value::CellVal::Str(schema.flags.join(" ")));
        batch.cells.push(shell_value::CellVal::Str(schema.columns.join(", ")));
        batch.cells.push(shell_value::CellVal::Str(schema.description.to_string()));
    }
    Ok(Value::from_batch(batch))
}
