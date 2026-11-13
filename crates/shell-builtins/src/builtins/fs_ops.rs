//! Filesystem operations shared by `rm`, `cp`, and `mv`.
//!
//! Each function does the work for one operand, never follows a symlink unless the
//! caller asked for it, and reports every failure through `Session::report` before
//! carrying on with the next entry. The return value says whether everything worked.
//! Undo capture (`record_preimage`, `record_tree`) runs before each mutation.

use std::fs::{self, FileTimes, Metadata};
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

use shell_kernel::Session;
use shell_value::StructuredDiagnostic;

use super::common::resolve_path;

/// An OS error as the coreutils print it, without Rust's ` (os error N)` suffix.
pub(super) fn describe(err: &io::Error) -> String {
    let text = err.to_string();
    match text.find(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

pub(super) fn fail(session: &mut Session, code: &'static str, message: String) -> bool {
    session.report(StructuredDiagnostic::new(code, message, None, None));
    false
}

pub(super) fn is_dir(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// Whether both paths name the same inode, resolving symlinks only when `follow` is set.
pub(super) fn same_file(a: &Path, b: &Path, follow: bool) -> bool {
    let stat = |p: &Path| {
        if follow {
            fs::metadata(p)
        } else {
            fs::symlink_metadata(p)
        }
    };
    match (stat(a), stat(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Resolves a path that may not exist yet: the deepest existing ancestor is
/// canonicalized and the missing tail is appended.
fn resolve_future(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut base = path;
    loop {
        if let Ok(mut real) = base.canonicalize() {
            real.extend(missing.iter().rev());
            return real;
        }
        match (base.file_name(), base.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                base = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether `dest` lies inside the directory `src` (or is it).
pub(super) fn is_inside(src: &Path, dest: &Path) -> bool {
    match src.canonicalize() {
        Ok(real_src) => resolve_future(dest).starts_with(real_src),
        Err(_) => false,
    }
}

/// The filesystem root and the session's workspace root are never removed.
pub(super) fn is_protected_root(session: &Session, path: &Path) -> bool {
    let Ok(real) = path.canonicalize() else {
        return false;
    };
    real.parent().is_none() || session.cow.as_ref().is_some_and(|c| real == c.root)
}

// ---------------------------------------------------------------- removal

pub(super) fn remove_leaf(
    session: &mut Session,
    code: &'static str,
    path: &Path,
    label: &str,
    verbose: bool,
    out: &mut Vec<u8>,
) -> bool {
    match fs::remove_file(path) {
        Ok(()) => {
            session.record_mutation(path.to_string_lossy().to_string());
            if verbose {
                out.extend_from_slice(format!("removed '{label}'\n").as_bytes());
            }
            true
        }
        Err(e) => fail(
            session,
            code,
            format!("cannot remove '{label}': {}", describe(&e)),
        ),
    }
}

pub(super) fn remove_empty_dir(
    session: &mut Session,
    code: &'static str,
    path: &Path,
    label: &str,
    verbose: bool,
    out: &mut Vec<u8>,
) -> bool {
    match fs::remove_dir(path) {
        Ok(()) => {
            session.record_mutation(path.to_string_lossy().to_string());
            if verbose {
                out.extend_from_slice(format!("removed directory '{label}'\n").as_bytes());
            }
            true
        }
        Err(e) => fail(
            session,
            code,
            format!("cannot remove '{label}': {}", describe(&e)),
        ),
    }
}

/// Removes a directory and everything below it. Symlinks are unlinked, never entered.
/// A failing entry is reported and the walk goes on; a directory that kept an entry stays.
pub(super) fn remove_tree(
    session: &mut Session,
    code: &'static str,
    path: &Path,
    label: &str,
    verbose: bool,
    out: &mut Vec<u8>,
) -> bool {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => {
            return fail(
                session,
                code,
                format!("cannot remove '{label}': {}", describe(&e)),
            );
        }
    };
    let mut ok = true;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                ok = fail(
                    session,
                    code,
                    format!("cannot remove '{label}': {}", describe(&e)),
                );
                continue;
            }
        };
        let child = entry.path();
        let child_label = format!(
            "{}/{}",
            label.trim_end_matches('/'),
            entry.file_name().to_string_lossy()
        );
        let is_real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        ok &= if is_real_dir {
            remove_tree(session, code, &child, &child_label, verbose, out)
        } else {
            remove_leaf(session, code, &child, &child_label, verbose, out)
        };
    }
    ok && remove_empty_dir(session, code, path, label, verbose, out)
}

// ---------------------------------------------------------------- copying

#[derive(Clone, Copy)]
pub(super) struct CopyOpts {
    pub recursive: bool,
    pub no_clobber: bool,
    pub force: bool,
    pub preserve: bool,
    /// Follow every symlink (`-L`).
    pub dereference: bool,
    /// Follow only symlinks named on the command line (cp without `-r`).
    pub follow_operands: bool,
    pub verbose: bool,
}

struct CopyCtx<'a> {
    code: &'static str,
    opts: CopyOpts,
    out: &'a mut Vec<u8>,
    /// (dev, ino) of the directories being copied, to stop a symlink cycle under `-L`.
    walking: Vec<(u64, u64)>,
}

/// Copies one operand. `src_label` and `dest_label` are how the user wrote them.
pub(super) fn copy_operand(
    session: &mut Session,
    code: &'static str,
    src: &Path,
    dest: &Path,
    labels: (&str, &str),
    opts: CopyOpts,
    out: &mut Vec<u8>,
) -> bool {
    let mut ctx = CopyCtx {
        code,
        opts,
        out,
        walking: Vec::new(),
    };
    copy_entry(session, &mut ctx, src, dest, labels, true)
}

fn copy_entry(
    session: &mut Session,
    ctx: &mut CopyCtx,
    src: &Path,
    dest: &Path,
    (src_label, dest_label): (&str, &str),
    operand: bool,
) -> bool {
    let follow = ctx.opts.dereference || (operand && ctx.opts.follow_operands);
    let meta = if follow {
        fs::metadata(src)
    } else {
        fs::symlink_metadata(src)
    };
    let meta = match meta {
        Ok(meta) => meta,
        Err(e) => {
            return fail(
                session,
                ctx.code,
                format!("cannot stat '{src_label}': {}", describe(&e)),
            );
        }
    };
    if meta.is_dir() {
        copy_dir(session, ctx, src, dest, (src_label, dest_label), &meta)
    } else {
        copy_leaf(session, ctx, src, dest, (src_label, dest_label), &meta)
    }
}

fn copy_dir(
    session: &mut Session,
    ctx: &mut CopyCtx,
    src: &Path,
    dest: &Path,
    (src_label, dest_label): (&str, &str),
    meta: &Metadata,
) -> bool {
    if !ctx.opts.recursive {
        return fail(
            session,
            ctx.code,
            format!("-r not specified; omitting directory '{src_label}'"),
        );
    }
    if ctx.walking.is_empty() && is_inside(src, dest) {
        return fail(
            session,
            ctx.code,
            format!("cannot copy a directory, '{src_label}', into itself, '{dest_label}'"),
        );
    }
    let id = (meta.dev(), meta.ino());
    if ctx.walking.contains(&id) {
        return fail(
            session,
            ctx.code,
            format!("cannot copy cyclic symbolic link '{src_label}'"),
        );
    }
    match fs::symlink_metadata(dest) {
        Ok(existing) if existing.is_dir() || is_dir(dest) => {}
        Ok(_) => {
            return fail(
                session,
                ctx.code,
                format!(
                    "cannot overwrite non-directory '{dest_label}' with directory '{src_label}'"
                ),
            );
        }
        Err(_) => {
            session.record_preimage(dest);
            if let Err(e) = fs::create_dir(dest) {
                return fail(
                    session,
                    ctx.code,
                    format!("cannot create directory '{dest_label}': {}", describe(&e)),
                );
            }
            session.record_mutation(dest.to_string_lossy().to_string());
            if ctx.opts.verbose {
                ctx.out
                    .extend_from_slice(format!("'{src_label}' -> '{dest_label}'\n").as_bytes());
            }
        }
    }

    let entries = match fs::read_dir(src) {
        Ok(entries) => entries,
        Err(e) => {
            return fail(
                session,
                ctx.code,
                format!("cannot open directory '{src_label}': {}", describe(&e)),
            );
        }
    };
    ctx.walking.push(id);
    let mut ok = true;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                ok = fail(
                    session,
                    ctx.code,
                    format!("cannot read directory '{src_label}': {}", describe(&e)),
                );
                continue;
            }
        };
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        let child_src_label = format!("{}/{name_text}", src_label.trim_end_matches('/'));
        let child_dest_label = format!("{}/{name_text}", dest_label.trim_end_matches('/'));
        ok &= copy_entry(
            session,
            ctx,
            &entry.path(),
            &dest.join(&name),
            (&child_src_label, &child_dest_label),
            false,
        );
    }
    ctx.walking.pop();
    if ctx.opts.preserve {
        ok &= preserve_attributes(session, ctx, dest, dest_label, meta);
    }
    ok
}

fn copy_leaf(
    session: &mut Session,
    ctx: &mut CopyCtx,
    src: &Path,
    dest: &Path,
    (src_label, dest_label): (&str, &str),
    meta: &Metadata,
) -> bool {
    let dest_meta = fs::symlink_metadata(dest).ok();
    if let Some(existing) = &dest_meta {
        if existing.is_dir() {
            return fail(
                session,
                ctx.code,
                format!("cannot overwrite directory '{dest_label}' with non-directory"),
            );
        }
        if same_file(src, dest, !meta.file_type().is_symlink()) {
            return fail(
                session,
                ctx.code,
                format!("'{src_label}' and '{dest_label}' are the same file"),
            );
        }
        if ctx.opts.no_clobber {
            return true;
        }
    }
    session.record_preimage(dest);

    let copied = if meta.file_type().is_symlink() {
        copy_symlink(src, dest, dest_meta.is_some())
    } else {
        copy_regular(src, dest, ctx.opts.force && dest_meta.is_some())
    };
    if let Err(e) = copied {
        return fail(
            session,
            ctx.code,
            format!(
                "cannot copy '{src_label}' to '{dest_label}': {}",
                describe(&e)
            ),
        );
    }
    session.record_mutation(dest.to_string_lossy().to_string());
    if ctx.opts.verbose {
        ctx.out
            .extend_from_slice(format!("'{src_label}' -> '{dest_label}'\n").as_bytes());
    }
    if ctx.opts.preserve && !meta.file_type().is_symlink() {
        return preserve_attributes(session, ctx, dest, dest_label, meta);
    }
    true
}

fn copy_symlink(src: &Path, dest: &Path, replace: bool) -> io::Result<()> {
    let target = fs::read_link(src)?;
    if replace {
        fs::remove_file(dest)?;
    }
    symlink(target, dest)
}

/// `fs::copy` also copies the permission bits. With `force`, a destination that cannot
/// be opened is removed and the copy retried, as `cp -f` does.
fn copy_regular(src: &Path, dest: &Path, force: bool) -> io::Result<()> {
    match fs::copy(src, dest) {
        Err(e) if force && e.kind() == io::ErrorKind::PermissionDenied => {
            fs::remove_file(dest)?;
            fs::copy(src, dest).map(drop)
        }
        other => other.map(drop),
    }
}

fn preserve_attributes(
    session: &mut Session,
    ctx: &CopyCtx,
    dest: &Path,
    dest_label: &str,
    meta: &Metadata,
) -> bool {
    let result = (|| {
        let mut times = FileTimes::new();
        if let Ok(modified) = meta.modified() {
            times = times.set_modified(modified);
        }
        if let Ok(accessed) = meta.accessed() {
            times = times.set_accessed(accessed);
        }
        fs::set_permissions(dest, meta.permissions())?;
        fs::File::open(dest)?.set_times(times)
    })();
    match result {
        Ok(()) => true,
        Err(e) => fail(
            session,
            ctx.code,
            format!(
                "failed to preserve attributes for '{dest_label}': {}",
                describe(&e)
            ),
        ),
    }
}

// ---------------------------------------------------------------- copy and move operands

/// One source and the path it lands on, with both as the user wrote them.
pub(super) struct Transfer {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub src_label: String,
    pub dest_label: String,
}

/// Resolves `SRC... DST` operands the way `cp` and `mv` do: with one source `DST` is the
/// new name, unless it is a directory, and with several sources it must be a directory.
pub(super) fn plan_transfers(
    session: &Session,
    cmd: &str,
    code: &'static str,
    operands: &[&str],
) -> Result<Vec<Transfer>, StructuredDiagnostic> {
    let usage = |message: String| {
        StructuredDiagnostic::new(
            "E_ARGS",
            message,
            None,
            Some(format!("usage: {cmd} [OPTION]... SOURCE... DEST")),
        )
    };
    let (target_label, sources) = match operands {
        [] => return Err(usage(format!("{cmd}: missing file operand"))),
        [only] => {
            return Err(usage(format!(
                "{cmd}: missing destination file operand after '{only}'"
            )));
        }
        [sources @ .., target] => (*target, sources),
    };
    let target = resolve_path(session, target_label);
    let into_dir = is_dir(&target);
    if sources.len() > 1 && !into_dir {
        return Err(StructuredDiagnostic::new(
            code,
            format!("target '{target_label}' is not a directory"),
            None,
            None,
        ));
    }
    Ok(sources
        .iter()
        .map(|&label| {
            let src = resolve_path(session, label);
            let (dest, dest_label) = match (into_dir, Path::new(label).file_name()) {
                (true, Some(name)) => (
                    target.join(name),
                    format!(
                        "{}/{}",
                        target_label.trim_end_matches('/'),
                        name.to_string_lossy()
                    ),
                ),
                _ => (target.clone(), target_label.to_string()),
            };
            Transfer {
                src,
                dest,
                src_label: label.to_string(),
                dest_label,
            }
        })
        .collect())
}

#[derive(Clone, Copy)]
pub(super) struct MoveOpts {
    pub no_clobber: bool,
    pub verbose: bool,
}

/// Renames `src` onto `dest`; across filesystems it copies, then removes the source.
pub(super) fn move_operand(
    session: &mut Session,
    code: &'static str,
    transfer: &Transfer,
    opts: MoveOpts,
    out: &mut Vec<u8>,
) -> bool {
    let Transfer {
        src,
        dest,
        src_label,
        dest_label,
    } = transfer;
    let src_meta = match fs::symlink_metadata(src) {
        Ok(meta) => meta,
        Err(e) => {
            return fail(
                session,
                code,
                format!("cannot stat '{src_label}': {}", describe(&e)),
            );
        }
    };
    let src_is_dir = src_meta.is_dir();
    let dest_meta = fs::symlink_metadata(dest).ok();
    if let Some(existing) = &dest_meta {
        if same_file(src, dest, false) {
            return fail(
                session,
                code,
                format!("'{src_label}' and '{dest_label}' are the same file"),
            );
        }
        if opts.no_clobber {
            return true;
        }
        if existing.is_dir() && !src_is_dir {
            return fail(
                session,
                code,
                format!("cannot overwrite directory '{dest_label}' with non-directory"),
            );
        }
        if !existing.is_dir() && src_is_dir {
            return fail(
                session,
                code,
                format!(
                    "cannot overwrite non-directory '{dest_label}' with directory '{src_label}'"
                ),
            );
        }
    }
    if src_is_dir && is_inside(src, dest) {
        return fail(
            session,
            code,
            format!("cannot move '{src_label}' to a subdirectory of itself, '{dest_label}'"),
        );
    }

    session.record_tree(src);
    session.record_tree(dest);
    let moved = match fs::rename(src, dest) {
        Err(e) if e.raw_os_error() == Some(nix::libc::EXDEV) => {
            move_across_devices(session, code, transfer, src_is_dir, out)
        }
        Err(e) => {
            return fail(
                session,
                code,
                format!(
                    "cannot move '{src_label}' to '{dest_label}': {}",
                    describe(&e)
                ),
            );
        }
        Ok(()) => true,
    };
    if moved {
        session.record_mutation(src.to_string_lossy().to_string());
        session.record_mutation(dest.to_string_lossy().to_string());
        if opts.verbose {
            out.extend_from_slice(format!("renamed '{src_label}' -> '{dest_label}'\n").as_bytes());
        }
    }
    moved
}

fn move_across_devices(
    session: &mut Session,
    code: &'static str,
    transfer: &Transfer,
    src_is_dir: bool,
    out: &mut Vec<u8>,
) -> bool {
    let Transfer {
        src,
        dest,
        src_label,
        dest_label,
    } = transfer;
    if src_is_dir && fs::read_dir(dest).is_ok_and(|mut entries| entries.next().is_some()) {
        return fail(
            session,
            code,
            format!("cannot move '{src_label}' to '{dest_label}': Directory not empty"),
        );
    }
    let opts = CopyOpts {
        recursive: true,
        no_clobber: false,
        force: true,
        preserve: true,
        dereference: false,
        follow_operands: false,
        verbose: false,
    };
    if !copy_operand(session, code, src, dest, (src_label, dest_label), opts, out) {
        return false;
    }
    let mut sink = Vec::new();
    if src_is_dir {
        remove_tree(session, code, src, src_label, false, &mut sink)
    } else {
        remove_leaf(session, code, src, src_label, false, &mut sink)
    }
}
