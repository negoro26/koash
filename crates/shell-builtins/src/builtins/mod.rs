pub(crate) mod common;

mod ast;
mod cat;
mod cd;
mod cp;
mod diag;
mod discover;
mod echo;
mod env;
mod export;
mod find;
mod first;
mod from;
mod grep;
mod head;
mod help;
mod js;
mod kill;
mod last;
mod ls;
mod mkdir;
mod mv;
mod patch;
mod prewalk;
mod ps;
mod pwd;
mod py;
mod refs;
mod rename;
mod rm;
mod rollback;
mod select;
mod sort_by;
mod sql;
mod stat;
mod tail;
mod to;
mod tx_log;
mod undo;
mod where_;
mod which;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, Placement, Signature};

/// Order is the listing order of `help`, `discover`, and the schemas endpoint.
pub static REGISTRY: &[&dyn Builtin] = &[
    &cd::Cd,
    &pwd::Pwd,
    &export::Export,
    &env::Env,
    &ls::Ls,
    &stat::Stat,
    &cat::Cat,
    &echo::Echo,
    &head::Head,
    &tail::Tail,
    &find::Find,
    &grep::Grep,
    &patch::Patch,
    &prewalk::Prewalk,
    &ast::Ast,
    &ps::Ps,
    &kill::Kill,
    &which::Which,
    &mkdir::Mkdir,
    &rm::Rm,
    &cp::Cp,
    &mv::Mv,
    &where_::Where,
    &select::Select,
    &sort_by::SortBy,
    &first::First,
    &last::Last,
    &from::FromFormat,
    &to::ToFormat,
    &discover::Discover,
    &undo::Undo,
    &rollback::Rollback,
    &tx_log::TxLog,
    &diag::Diag,
    &refs::Refs,
    &rename::Rename,
    &py::Py,
    &js::Js,
    &sql::Sql,
    &help::Help,
];

pub fn lookup(name: &str) -> Option<&'static dyn Builtin> {
    REGISTRY.iter().copied().find(|b| {
        let sig = b.signature();
        sig.name == name || sig.aliases.contains(&name)
    })
}

/// Commands listed by help, discover, and the schemas endpoint.
/// Shell-control commands (`Placement::SoleStage`) are not part of the catalog.
pub fn catalog() -> impl Iterator<Item = &'static Signature> {
    REGISTRY
        .iter()
        .map(|b| b.signature())
        .filter(|s| s.placement == Placement::Any)
}

pub fn run_builtin(
    session: &mut Session,
    name: &str,
    args: &[String],
    input: Value,
) -> Result<Value, StructuredDiagnostic> {
    match lookup(name) {
        Some(builtin) => builtin.run(session, args, input),
        None => Err(StructuredDiagnostic::new(
            "E_UNKNOWN_BUILTIN",
            format!("unknown builtin `{name}`"),
            None,
            None,
        )),
    }
}

pub(crate) fn reads_stdin(name: &str, args: &[String]) -> bool {
    lookup(name).is_some_and(|b| b.reads_input(args))
}
