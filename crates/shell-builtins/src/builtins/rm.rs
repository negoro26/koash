use std::fs;

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::resolve_path;
use super::fs_ops::{
    describe, fail, is_protected_root, remove_empty_dir, remove_leaf, remove_tree,
};
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Rm;

static SIGNATURE: Signature = Signature {
    name: "rm",
    aliases: &[],
    description: "Remove files or directories",
    flags: &[
        Flag::switch('r', "recursive", "remove directories and their contents"),
        Flag::short_switch('R', "same as -r"),
        Flag::switch('f', "force", "ignore operands that do not exist"),
        Flag::switch('d', "dir", "remove empty directories"),
        Flag::switch('v', "verbose", "say what was removed"),
    ],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Rm {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        if args.operands.is_empty() {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "rm: missing operand",
                None,
                Some("usage: rm [-rfdv] FILE...".to_string()),
            ));
        }
        let opts = Opts {
            recursive: args.has("r") || args.has("R"),
            force: args.has("f"),
            dir: args.has("d"),
            verbose: args.has("v"),
        };
        let mut out = Vec::new();
        for operand in &args.operands {
            remove_operand(session, operand, opts, &mut out);
        }
        Ok(Value::from_bytes(out))
    }
}

#[derive(Clone, Copy)]
struct Opts {
    recursive: bool,
    force: bool,
    dir: bool,
    verbose: bool,
}

/// Mirrors uutils `rm` (`remove` and `handle_dir` in rm.rs): `symlink_metadata` decides what an
/// operand is, so a symlink is unlinked and never followed, and a directory needs -r or -d.
fn remove_operand(session: &mut Session, operand: &str, opts: Opts, out: &mut Vec<u8>) {
    let path = resolve_path(session, operand);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(_) if opts.force => return,
        Err(e) => {
            fail(
                session,
                "E_RM",
                format!("cannot remove '{operand}': {}", describe(&e)),
            );
            return;
        }
    };
    if !meta.is_dir() {
        session.record_tree(&path);
        remove_leaf(session, "E_RM", &path, operand, opts.verbose, out);
        return;
    }
    if names_dot_or_dotdot(operand) {
        fail(
            session,
            "E_RM",
            format!("refusing to remove '.' or '..' directory: skipping '{operand}'"),
        );
        return;
    }
    if (opts.recursive || opts.dir) && is_protected_root(session, &path) {
        fail(
            session,
            "E_RM",
            format!(
                "it is dangerous to operate recursively on '{operand}'; koa never removes the filesystem root"
            ),
        );
        return;
    }
    if opts.recursive {
        session.record_tree(&path);
        remove_tree(session, "E_RM", &path, operand, opts.verbose, out);
    } else if opts.dir {
        session.record_preimage(&path);
        remove_empty_dir(session, "E_RM", &path, operand, opts.verbose, out);
    } else {
        fail(
            session,
            "E_RM",
            format!("cannot remove '{operand}': Is a directory"),
        );
    }
}

fn names_dot_or_dotdot(operand: &str) -> bool {
    let trimmed = operand.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    last == "." || last == ".."
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use super::*;

    #[test]
    fn dot_and_dotdot_are_recognised_with_trailing_slashes() {
        for operand in [".", "..", "./", "a/.", "a/..", "a/../"] {
            assert!(names_dot_or_dotdot(operand), "{operand}");
        }
        for operand in ["a", ".hidden", "..x", "a/b."] {
            assert!(!names_dot_or_dotdot(operand), "{operand}");
        }
    }

    fn session() -> Session {
        Session::new(false).unwrap()
    }

    #[test]
    fn the_filesystem_root_and_symlinks_to_it_are_protected() {
        let dir = std::env::temp_dir().join(format!("koa-rm-root-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        symlink("/", dir.join("rootlink")).unwrap();
        let s = session();
        assert!(is_protected_root(&s, Path::new("/")));
        assert!(is_protected_root(&s, Path::new("/.")));
        assert!(is_protected_root(&s, Path::new("/usr/..")));
        assert!(is_protected_root(&s, &dir.join("rootlink/")));
        assert!(!is_protected_root(&s, &dir));
        assert!(!is_protected_root(&s, Path::new("/nonexistent-koa-path")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_workspace_root_of_a_cow_session_is_protected() {
        let root = std::env::temp_dir().join(format!("koa-rm-cow-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut s = session();
        s.cow = shell_kernel::CowWorkspace::new(&root, "rm-test").ok();
        assert!(s.cow.is_some());
        let real = root.canonicalize().unwrap();
        assert!(is_protected_root(&s, &real));
        assert!(!is_protected_root(&s, &real.join(".koa")));
        drop(s);
        fs::remove_dir_all(&root).unwrap();
    }
}
