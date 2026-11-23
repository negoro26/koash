pub(crate) mod common;
mod fs_ops;
mod slice;

mod alias;
mod ast;
mod bg;
mod cat;
mod cd;
mod cp;
mod diag;
mod dirstack;
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
mod history;
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
mod redo;
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
mod type_;
mod unalias;
mod undo;
mod unset;
mod wait;
mod where_;
mod which;

use crate::builtin::{Builtin, Placement, Signature};

pub static REGISTRY: &[&dyn Builtin] = &[
    &cd::Cd,
    &dirstack::Pushd,
    &dirstack::Popd,
    &dirstack::Dirs,
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
    &type_::TypeCmd,
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
    &redo::Redo,
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
    &wait::Wait,
    &alias::Alias,
    &unalias::Unalias,
    &history::HistoryList,
];

pub fn lookup(name: &str) -> Option<&'static dyn Builtin> {
    REGISTRY.iter().copied().find(|b| {
        let sig = b.signature();
        sig.name == name || sig.aliases.contains(&name)
    })
}

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
}
