use std::collections::HashMap;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::fs::{self, Metadata};
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use shell_kernel::Session;
use shell_surface::terminal_width;
use shell_value::{StructuredDiagnostic, display_width};

use super::common::resolve_path;
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::Signature;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Columns,
    One,
    Long,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Name,
    Time,
    Size,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShowDots {
    None,
    AlmostAll,
    All,
}

struct Entry {
    name: String,
    quoted: String,
    width: usize,
    meta: Metadata,
    target: Option<String>,
    target_meta: Option<Metadata>,
}

struct Colors {
    enabled: bool,
    long: bool,
    used: std::cell::Cell<bool>,
    lc: String,
    rc: String,
    rs: String,
    no: Option<String>,
    fi: Option<String>,
    di: Option<String>,
    ln: Option<String>,
    ln_target: bool,
    pi: Option<String>,
    so: Option<String>,
    bd: Option<String>,
    cd: Option<String>,
    or: Option<String>,
    su: Option<String>,
    sg: Option<String>,
    tw: Option<String>,
    ow: Option<String>,
    st: Option<String>,
    ex: Option<String>,
    exts: Vec<(String, String)>,
}

impl Colors {
    fn from_session(session: &Session) -> Self {
        let no_color = session.env.get("NO_COLOR").is_some_and(|v| !v.is_empty());
        let term = session.env.get("TERM");
        let enabled = !no_color
            && term
                .as_deref()
                .is_some_and(|t| !t.is_empty() && t != "dumb");
        let mut c = Self {
            enabled,
            long: false,
            used: std::cell::Cell::new(false),
            lc: "\x1b[".to_string(),
            rc: "m".to_string(),
            rs: "0".to_string(),
            no: None,
            fi: None,
            di: Some("01;34".to_string()),
            ln: Some("01;36".to_string()),
            ln_target: false,
            pi: Some("33".to_string()),
            so: Some("01;35".to_string()),
            bd: Some("01;33".to_string()),
            cd: Some("01;33".to_string()),
            or: Some("01;31".to_string()),
            su: Some("01;31;4".to_string()),
            sg: Some("01;33;4".to_string()),
            tw: Some("01;34;4".to_string()),
            ow: Some("01;34;4".to_string()),
            st: Some("01;34;4".to_string()),
            ex: Some("01;32".to_string()),
            exts: Vec::new(),
        };
        if !enabled {
            return c;
        }
        if let Some(spec) = session.env.get("LS_COLORS") {
            if spec.is_empty() {
                c.enabled = false;
                return c;
            }
            c.di = None;
            c.ln = None;
            c.pi = None;
            c.so = None;
            c.bd = None;
            c.cd = None;
            c.or = None;
            c.su = None;
            c.sg = None;
            c.tw = None;
            c.ow = None;
            c.st = None;
            c.ex = None;
            for part in spec.split(':') {
                let Some((k, v)) = part.split_once('=') else {
                    continue;
                };
                let val = if v.is_empty() {
                    None
                } else {
                    Some(unescape_color(v))
                };
                match k {
                    "lc" => {
                        if let Some(s) = val {
                            c.lc = s;
                        }
                    }
                    "rc" => {
                        if let Some(s) = val {
                            c.rc = s;
                        }
                    }
                    "rs" => {
                        if let Some(s) = val {
                            c.rs = s;
                        }
                    }
                    "no" => c.no = val,
                    "fi" => c.fi = val,
                    "di" => c.di = val,
                    "ln" => {
                        if val.as_deref() == Some("target") {
                            c.ln_target = true;
                            c.ln = None;
                        } else {
                            c.ln_target = false;
                            c.ln = val;
                        }
                    }
                    "pi" => c.pi = val,
                    "so" => c.so = val,
                    "bd" => c.bd = val,
                    "cd" => c.cd = val,
                    "or" => c.or = val,
                    "su" => c.su = val,
                    "sg" => c.sg = val,
                    "tw" => c.tw = val,
                    "ow" => c.ow = val,
                    "st" => c.st = val,
                    "ex" => c.ex = val,
                    _ if k.starts_with('*') => {
                        if let Some(s) = val {
                            c.exts.push((k[1..].to_string(), s));
                        }
                    }
                    _ => {}
                }
            }
        }
        c
    }

    fn code_for(&self, entry: &Entry) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        let ft = entry.meta.file_type();
        if ft.is_symlink() {
            if self.ln_target {
                return match &entry.target_meta {
                    Some(tmeta) => {
                        self.code_for_meta(tmeta, entry.target.as_deref().unwrap_or(&entry.name))
                    }
                    None => self.or.as_deref().or(self.ln.as_deref()),
                };
            }
            return self.ln.as_deref();
        }
        self.code_for_meta(&entry.meta, &entry.name)
    }

    fn code_for_meta(&self, meta: &Metadata, name: &str) -> Option<&str> {
        let ft = meta.file_type();
        let mode = meta.permissions().mode();
        if ft.is_dir() {
            if (mode & 0o1002) == 0o1002 {
                return self.tw.as_deref().or(self.di.as_deref());
            }
            if (mode & 0o0002) != 0 {
                return self.ow.as_deref().or(self.di.as_deref());
            }
            if (mode & 0o1000) != 0 {
                return self.st.as_deref().or(self.di.as_deref());
            }
            return self.di.as_deref();
        }
        if ft.is_fifo() {
            return self.pi.as_deref();
        }
        if ft.is_socket() {
            return self.so.as_deref();
        }
        if ft.is_block_device() {
            return self.bd.as_deref();
        }
        if ft.is_char_device() {
            return self.cd.as_deref();
        }
        if (mode & 0o4000) != 0
            && let Some(s) = self.su.as_deref()
        {
            return Some(s);
        }
        if (mode & 0o2000) != 0
            && let Some(s) = self.sg.as_deref()
        {
            return Some(s);
        }
        if (mode & 0o0111) != 0
            && let Some(s) = self.ex.as_deref()
        {
            return Some(s);
        }
        for (suffix, code) in self.exts.iter().rev() {
            if name.ends_with(suffix.as_str()) {
                return Some(code.as_str());
            }
        }
        self.fi.as_deref().or(self.no.as_deref())
    }

    fn write_colored(&self, out: &mut Vec<u8>, text: &str, code: Option<&str>) {
        if let Some(code) = code {
            if !self.used.replace(true) {
                let _ = write!(out, "{}{}{}", self.lc, self.rs, self.rc);
            }
            let _ = write!(
                out,
                "{}{code}{}{text}{}{}{}",
                self.lc, self.rc, self.lc, self.rs, self.rc
            );
        } else {
            out.extend_from_slice(text.as_bytes());
        }
    }
}

fn unescape_color(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('e' | 'E') => out.push('\x1b'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

struct IdCache {
    users: HashMap<u32, String>,
    groups: HashMap<u32, String>,
}

impl IdCache {
    fn new() -> Self {
        Self {
            users: HashMap::new(),
            groups: HashMap::new(),
        }
    }

    fn user(&mut self, uid: u32) -> &str {
        self.users
            .entry(uid)
            .or_insert_with(|| lookup_user(uid).unwrap_or_else(|| uid.to_string()))
    }

    fn group(&mut self, gid: u32) -> &str {
        self.groups
            .entry(gid)
            .or_insert_with(|| lookup_group(gid).unwrap_or_else(|| gid.to_string()))
    }
}

fn lookup_user(uid: u32) -> Option<String> {
    let mut pwd: nix::libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 1024];
    let mut result: *mut nix::libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        nix::libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc == 0 && !result.is_null() && !pwd.pw_name.is_null() {
        let name = unsafe { CStr::from_ptr(pwd.pw_name) };
        return Some(name.to_string_lossy().into_owned());
    }
    None
}

fn lookup_group(gid: u32) -> Option<String> {
    let mut grp: nix::libc::group = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 1024];
    let mut result: *mut nix::libc::group = std::ptr::null_mut();
    let rc = unsafe {
        nix::libc::getgrgid_r(
            gid,
            &mut grp,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc == 0 && !result.is_null() && !grp.gr_name.is_null() {
        let name = unsafe { CStr::from_ptr(grp.gr_name) };
        return Some(name.to_string_lossy().into_owned());
    }
    None
}

pub(super) fn render(
    sig: &'static Signature,
    session: &mut Session,
    raw_args: &[String],
    columns_env: Option<&str>,
) -> Result<Vec<u8>, StructuredDiagnostic> {
    let args = Args::parse(sig, raw_args)?;
    let layout = if args.has("l") {
        Layout::Long
    } else if args.has("1") {
        Layout::One
    } else {
        Layout::Columns
    };
    let sort = match args.last_of(&["t", "S"]) {
        Some("t") => SortMode::Time,
        Some("S") => SortMode::Size,
        _ => SortMode::Name,
    };
    let dots = match args.last_of(&["a", "A"]) {
        Some("a") => ShowDots::All,
        Some("A") => ShowDots::AlmostAll,
        _ => ShowDots::None,
    };
    let reverse = args.has("reverse");
    let human = args.has("human-readable");
    let width = terminal_width(columns_env).unwrap_or(80);
    let mut colors = Colors::from_session(session);
    colors.long = layout == Layout::Long;
    let mut ids = IdCache::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);

    let targets: Vec<&str> = if args.operands.is_empty() {
        vec!["."]
    } else {
        args.operands.clone()
    };
    let multi = targets.len() > 1;

    let mut file_entries = Vec::new();
    let mut dir_targets: Vec<(String, PathBuf)> = Vec::new();

    for target in targets {
        let resolved = resolve_path(session, target);
        let follow = layout != Layout::Long || target.ends_with('/');
        let is_dir = if follow {
            fs::metadata(&resolved).is_ok_and(|m| m.is_dir())
        } else {
            fs::symlink_metadata(&resolved).is_ok_and(|m| m.is_dir())
        };
        if is_dir {
            dir_targets.push((target.to_string(), resolved));
            continue;
        }
        match load_entry(target, &resolved) {
            Ok(entry) => file_entries.push(entry),
            Err(e) => {
                fail(
                    session,
                    "E_LS",
                    format!("cannot access '{target}': {}", describe(&e)),
                );
            }
        }
    }

    sort_entries(&mut file_entries, sort, reverse);
    dir_targets.sort_by(|a, b| {
        if reverse {
            b.0.cmp(&a.0)
        } else {
            a.0.cmp(&b.0)
        }
    });

    let mut out = Vec::new();
    let mut printed_any = false;

    if !file_entries.is_empty() {
        render_entries(
            &file_entries,
            layout,
            human,
            width,
            &colors,
            &mut ids,
            now,
            false,
            &mut out,
        );
        printed_any = true;
    }

    for (label, path) in dir_targets {
        let mut entries = match read_dir_entries(&path, dots) {
            Ok(e) => e,
            Err(e) => {
                fail(
                    session,
                    "E_LS",
                    format!("cannot open directory '{label}': {}", describe(&e)),
                );
                continue;
            }
        };
        if printed_any {
            out.push(b'\n');
        }
        if multi {
            let _ = writeln!(out, "{}:", quote_name(&label));
        }
        sort_entries(&mut entries, sort, reverse);
        render_entries(
            &entries, layout, human, width, &colors, &mut ids, now, true, &mut out,
        );
        printed_any = true;
    }

    Ok(out)
}

fn load_entry(name: &str, path: &Path) -> std::io::Result<Entry> {
    let meta = fs::symlink_metadata(path)?;
    let (target, target_meta) = if meta.file_type().is_symlink() {
        let link = fs::read_link(path)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        let tmeta = fs::metadata(path).ok();
        (link, tmeta)
    } else {
        (None, None)
    };
    let quoted = quote_name(name);
    let width = display_width(&quoted);
    Ok(Entry {
        name: name.to_string(),
        quoted,
        width,
        meta,
        target,
        target_meta,
    })
}

fn read_dir_entries(dir: &Path, dots: ShowDots) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    if dots == ShowDots::All {
        if let Ok(e) = load_entry(".", dir) {
            entries.push(e);
        }
        if let Ok(e) = load_entry("..", &dir.join("..")) {
            entries.push(e);
        }
    }
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let fname = item.file_name();
        let name = fname.to_string_lossy();
        if dots == ShowDots::None && name.starts_with('.') {
            continue;
        }
        if let Ok(entry) = load_entry(&name, &item.path()) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn sort_entries(entries: &mut [Entry], sort: SortMode, reverse: bool) {
    entries.sort_by(|a, b| {
        let ord = match sort {
            SortMode::Name => a.name.cmp(&b.name),
            SortMode::Time => {
                let ta = (a.meta.mtime(), a.meta.mtime_nsec());
                let tb = (b.meta.mtime(), b.meta.mtime_nsec());
                tb.cmp(&ta).then_with(|| a.name.cmp(&b.name))
            }
            SortMode::Size => b
                .meta
                .len()
                .cmp(&a.meta.len())
                .then_with(|| a.name.cmp(&b.name)),
        };
        if reverse { ord.reverse() } else { ord }
    });
}

#[allow(clippy::too_many_arguments)]
fn render_entries(
    entries: &[Entry],
    layout: Layout,
    human: bool,
    width: usize,
    colors: &Colors,
    ids: &mut IdCache,
    now: i64,
    show_total: bool,
    out: &mut Vec<u8>,
) {
    match layout {
        Layout::One => {
            for e in entries {
                colors.write_colored(out, &e.quoted, colors.code_for(e));
                out.push(b'\n');
            }
        }
        Layout::Columns => render_columns(entries, width, colors, out),
        Layout::Long => render_long(entries, human, colors, ids, now, show_total, out),
    }
}

fn render_columns(entries: &[Entry], max_width: usize, colors: &Colors, out: &mut Vec<u8>) {
    if entries.is_empty() {
        return;
    }
    let n = entries.len();
    let mut best_cols = 1usize;
    let mut best_widths = vec![entries.iter().map(|e| e.width).max().unwrap_or(0)];

    for ncols in 2..=n {
        let nrows = n.div_ceil(ncols);
        let actual_cols = n.div_ceil(nrows);
        if actual_cols < ncols {
            continue;
        }
        let mut col_widths = vec![0usize; actual_cols];
        for (idx, e) in entries.iter().enumerate() {
            let c = idx / nrows;
            if e.width > col_widths[c] {
                col_widths[c] = e.width;
            }
        }
        let total: usize = col_widths.iter().sum::<usize>() + (actual_cols - 1) * 2;
        if total <= max_width {
            best_cols = actual_cols;
            best_widths = col_widths;
        } else {
            break;
        }
    }

    let nrows = n.div_ceil(best_cols);
    for r in 0..nrows {
        for (c, &col_w) in best_widths.iter().enumerate() {
            let idx = c * nrows + r;
            if idx >= n {
                break;
            }
            let e = &entries[idx];
            colors.write_colored(out, &e.quoted, colors.code_for(e));
            let next_exists = (c + 1) * nrows + r < n;
            if next_exists {
                let pad = col_w.saturating_sub(e.width) + 2;
                for _ in 0..pad {
                    out.push(b' ');
                }
            }
        }
        out.push(b'\n');
    }
}

fn render_long(
    entries: &[Entry],
    human: bool,
    colors: &Colors,
    ids: &mut IdCache,
    now: i64,
    show_total: bool,
    out: &mut Vec<u8>,
) {
    if show_total {
        let blocks: u64 = entries.iter().map(|e| e.meta.blocks()).sum::<u64>() / 2;
        let _ = writeln!(out, "total {blocks}");
    }
    if entries.is_empty() {
        return;
    }

    let mut rows_meta: Vec<(String, String, String, String, String, String)> =
        Vec::with_capacity(entries.len());
    let (mut w_nlink, mut w_user, mut w_group, mut w_size) = (0usize, 0usize, 0usize, 0usize);

    for e in entries {
        let mode = format_mode(&e.meta);
        let nlink = e.meta.nlink().to_string();
        let user = ids.user(e.meta.uid()).to_string();
        let group = ids.group(e.meta.gid()).to_string();
        let size = format_size(&e.meta, human);
        let mtime = format_mtime(e.meta.mtime(), now);
        w_nlink = w_nlink.max(nlink.len());
        w_user = w_user.max(user.len());
        w_group = w_group.max(group.len());
        w_size = w_size.max(size.len());
        rows_meta.push((mode, nlink, user, group, size, mtime));
    }

    for (e, (mode, nlink, user, group, size, mtime)) in entries.iter().zip(rows_meta.iter()) {
        let _ = write!(
            out,
            "{mode} {nlink:>w_nlink$} {user:<w_user$} {group:<w_group$} {size:>w_size$} {mtime} "
        );
        colors.write_colored(out, &e.quoted, colors.code_for(e));
        if let Some(target) = &e.target {
            out.extend_from_slice(b" -> ");
            let tcode = (colors.enabled && colors.ln_target).then(|| {
                e.target_meta
                    .as_ref()
                    .and_then(|tm| colors.code_for_meta(tm, target))
            });
            colors.write_colored(out, target, tcode.flatten());
        }
        out.push(b'\n');
    }
}

fn format_mode(meta: &Metadata) -> String {
    let ft = meta.file_type();
    let kind = if ft.is_dir() {
        'd'
    } else if ft.is_symlink() {
        'l'
    } else if ft.is_fifo() {
        'p'
    } else if ft.is_socket() {
        's'
    } else if ft.is_block_device() {
        'b'
    } else if ft.is_char_device() {
        'c'
    } else {
        '-'
    };
    let m = meta.permissions().mode();
    let mut s = String::with_capacity(10);
    s.push(kind);
    s.push(if m & 0o400 != 0 { 'r' } else { '-' });
    s.push(if m & 0o200 != 0 { 'w' } else { '-' });
    s.push(match (m & 0o100 != 0, m & 0o4000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });
    s.push(if m & 0o040 != 0 { 'r' } else { '-' });
    s.push(if m & 0o020 != 0 { 'w' } else { '-' });
    s.push(match (m & 0o010 != 0, m & 0o2000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });
    s.push(if m & 0o004 != 0 { 'r' } else { '-' });
    s.push(if m & 0o002 != 0 { 'w' } else { '-' });
    s.push(match (m & 0o001 != 0, m & 0o1000 != 0) {
        (true, true) => 't',
        (false, true) => 'T',
        (true, false) => 'x',
        (false, false) => '-',
    });
    s
}

fn format_size(meta: &Metadata, human: bool) -> String {
    let ft = meta.file_type();
    if ft.is_char_device() || ft.is_block_device() {
        let rdev = meta.rdev();
        let maj = nix::sys::stat::major(rdev);
        let min = nix::sys::stat::minor(rdev);
        return format!("{maj}, {min:>3}");
    }
    let bytes = meta.len();
    if !human || bytes < 1024 {
        return bytes.to_string();
    }
    const UNITS: [char; 6] = ['K', 'M', 'G', 'T', 'P', 'E'];
    let mut val = bytes as f64;
    let mut unit = 'K';
    for u in UNITS {
        val /= 1024.0;
        unit = u;
        if val < 1024.0 {
            break;
        }
    }
    if val < 9.95 {
        format!("{:.1}{unit}", (val * 10.0).ceil() / 10.0)
    } else {
        format!("{:.0}{unit}", val.ceil())
    }
}

fn format_mtime(secs: i64, now: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut tm: nix::libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as nix::libc::time_t;
    unsafe {
        nix::libc::localtime_r(&t, &mut tm);
    }
    let mon = MONTHS.get(tm.tm_mon as usize).copied().unwrap_or("Jan");
    let day = tm.tm_mday;
    let six_months = 31_556_952 / 2;
    if (now - secs).abs() <= six_months {
        format!("{mon} {day:>2} {:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        format!("{mon} {day:>2} {:>5}", tm.tm_year + 1900)
    }
}

fn quote_name(name: &str) -> String {
    let needs_quote = name.starts_with(['#', '~'])
        || name.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    ' ' | '!'
                        | '"'
                        | '$'
                        | '&'
                        | '\''
                        | '('
                        | ')'
                        | '*'
                        | ';'
                        | '<'
                        | '='
                        | '>'
                        | '?'
                        | '['
                        | '\\'
                        | '^'
                        | '`'
                        | '{'
                        | '|'
                        | '}'
                )
        });
    if !needs_quote {
        return name.to_string();
    }
    if !name.contains('\'') && !name.chars().any(char::is_control) {
        return format!("'{name}'");
    }
    if !name.contains('"')
        && !name.contains('$')
        && !name.contains('`')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
    {
        return format!("\"{name}\"");
    }
    let mut out = String::with_capacity(name.len() + 4);
    out.push_str("$'");
    for c in name.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}
