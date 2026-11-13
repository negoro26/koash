pub(crate) mod common;

mod ast;
mod bg;
mod cat;
mod cd;
mod cp;
mod diag;
mod discover;
mod echo;
mod env;
mod exit;
mod export;
mod fg;
mod find;
mod first;
mod from;
mod grep;
mod head;
mod help;
mod jobs;
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
mod unset;
mod where_;
mod which;

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
    &unset::Unset,
    &exit::Exit,
    &jobs::Jobs,
    &fg::Fg,
    &bg::Bg,
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_name_and_alias_is_unique() {
        let mut seen = HashSet::new();
        for builtin in REGISTRY {
            let sig = builtin.signature();
            for name in std::iter::once(sig.name).chain(sig.aliases.iter().copied()) {
                assert!(seen.insert(name), "duplicate registry name `{name}`");
            }
        }
    }

    #[test]
    fn lookup_resolves_aliases_to_the_canonical_command() {
        assert_eq!(
            lookup("sort_by").map(|b| b.signature().name),
            Some("sort-by")
        );
        assert_eq!(lookup("tx_log").map(|b| b.signature().name), Some("tx-log"));
        assert!(lookup("no-such-command").is_none());
    }

    #[test]
    fn catalog_leaves_out_sole_stage_commands() {
        assert!(catalog().all(|s| s.placement == Placement::Any));
        assert!(lookup("jobs").is_some_and(|b| b.signature().placement == Placement::SoleStage));
        assert!(catalog().all(|s| s.name != "jobs"));
    }
}
