use rustc_hash::FxHashMap;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{DirEntry, Metadata};
use std::io;
use std::ops::Range;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::natord::{compare_ignore_case, sort_key};
use super::style::{Painter, Style};
use super::tables::{FILE, FILE_UNKNOWN, FOLDER, FOLDER_OPEN};
use super::theme::{Theme, directory_icon, extension_icon, filename_icon};

pub(crate) struct Environment {
    pub no_color: bool,
    pub ls_colors: Option<String>,
    pub eza_colors: Option<String>,
    pub icon_spacing: usize,
    pub columns: Option<usize>,
}

#[derive(Clone, Copy)]
pub(crate) enum Sort {
    Name,
    Newest,
    Largest,
}

pub(crate) struct Settings {
    pub sort: Sort,
    pub reverse: bool,
    pub lines: bool,
}

pub(crate) struct Entry {
    name: String,
    typed: Option<String>,
    path: PathBuf,
    meta: Metadata,
    xattr: bool,
    key: Option<Box<[u8]>>,
}

impl Entry {
    pub(crate) fn operand(typed: &str, path: PathBuf, meta: Metadata, with_xattr: bool) -> Entry {
        let xattr = with_xattr && has_xattrs(&path, &mut Vec::new());
        let name = file_name_of(Path::new(typed));
        Entry {
            key: sort_key(&name),
            name,
            typed: Some(typed.to_string()),
            path,
            meta,
            xattr,
        }
    }

    fn child(
        dir: &Path,
        entry: &DirEntry,
        with_xattr: bool,
        scratch: &mut Vec<u8>,
    ) -> Result<Entry, (OsString, io::Error)> {
        let name = entry.file_name();
        let meta = entry.metadata().map_err(|e| (name.clone(), e))?;
        let path = dir.join(&name);
        let xattr = with_xattr && has_xattrs(&path, scratch);
        let name = name.to_string_lossy().into_owned();
        Ok(Entry {
            key: sort_key(&name),
            name,
            typed: None,
            path,
            meta,
            xattr,
        })
    }
}

pub(crate) fn read_children(
    dir: &Path,
    entries: &[DirEntry],
    with_xattr: bool,
) -> Vec<Result<Entry, (OsString, io::Error)>> {
    parallel_ranges(entries.len(), 512, |range| {
        let mut scratch = Vec::new();
        entries[range]
            .iter()
            .map(|entry| Entry::child(dir, entry, with_xattr, &mut scratch))
            .collect::<Vec<_>>()
    })
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) struct Folder {
    pub heading: String,
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

pub(crate) struct Layout {
    pub files: Vec<Entry>,
    pub folders: Vec<Folder>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Pipe,
    Link,
    Char,
    Block,
    Socket,
    Special,
}

fn kind_of(meta: &Metadata) -> Kind {
    let t = meta.file_type();
    if t.is_file() {
        Kind::File
    } else if t.is_dir() {
        Kind::Dir
    } else if t.is_fifo() {
        Kind::Pipe
    } else if t.is_symlink() {
        Kind::Link
    } else if t.is_char_device() {
        Kind::Char
    } else if t.is_block_device() {
        Kind::Block
    } else if t.is_socket() {
        Kind::Socket
    } else {
        Kind::Special
    }
}

struct Target {
    raw: PathBuf,
    meta: Metadata,
    mount: bool,
}

enum Link {
    None,
    Ok(Box<Target>),
    Broken(PathBuf),
    Unreadable,
}

struct Row<'e> {
    entry: &'e Entry,
    kind: Kind,
    link: Link,
    dirlike: bool,
    mount: bool,
    xattr: bool,
}

#[derive(Clone, Copy, Default)]
struct Text {
    buf: [u8; 24],
    len: u8,
}

impl Text {
    fn bytes(&self) -> &[u8] {
        &self.buf[..usize::from(self.len)]
    }

    fn width(&self) -> usize {
        usize::from(self.len)
    }

    fn push_byte(&mut self, byte: u8) {
        if let Some(slot) = self.buf.get_mut(usize::from(self.len)) {
            *slot = byte;
            self.len += 1;
        }
    }

    fn push_str(&mut self, text: &str) {
        text.bytes().for_each(|byte| self.push_byte(byte));
    }

    fn push_two_digits(&mut self, n: i64, pad: u8) {
        self.push_byte(if n < 10 {
            pad
        } else {
            b'0' + (n / 10 % 10) as u8
        });
        self.push_byte(b'0' + (n % 10) as u8);
    }
}

impl std::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let at = usize::from(self.len);
        let end = at + s.len();
        let slot = self.buf.get_mut(at..end).ok_or(std::fmt::Error)?;
        slot.copy_from_slice(s.as_bytes());
        self.len = end as u8;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SizeCell {
    Dash,
    Number(Text),
    Prefixed { number: Text, unit: u8, slot: usize },
    Device { major: u32, minor: u32 },
}

#[derive(Clone, Copy)]
struct Cells {
    size: SizeCell,
    user: usize,
    date: Text,
}

struct UserName {
    name: String,
    width: usize,
    style: Style,
}

struct Siblings<'a> {
    entries: &'a [Entry],
    names: OnceLock<HashSet<&'a str>>,
    enabled: bool,
}

impl Siblings<'_> {
    fn contains(&self, name: &str) -> bool {
        self.enabled
            && self
                .names
                .get_or_init(|| self.entries.iter().map(|e| e.name.as_str()).collect())
                .contains(name)
    }
}

struct Ctx<'s> {
    theme: Theme,
    icons: Option<usize>,
    settings: &'s Settings,
    offset: i64,
    year: i64,
    me: u32,
    users: Vec<UserName>,
    user_index: HashMap<u32, usize>,
    last_user: (u32, usize),
    mounts: OnceLock<HashSet<PathBuf>>,
}

fn terminal_width() -> Option<usize> {
    let mut size = nix::libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { nix::libc::ioctl(1, nix::libc::TIOCGWINSZ, &mut size) };
    (rc == 0 && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

fn local_offset(now: i64) -> i64 {
    unsafe {
        let mut tm: nix::libc::tm = std::mem::zeroed();
        let secs: nix::libc::time_t = now;
        if nix::libc::localtime_r(&secs, &mut tm).is_null() {
            0
        } else {
            tm.tm_gmtoff
        }
    }
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn printable(c: char) -> bool {
    c >= '\u{20}' && c != '\u{7f}'
}

fn escape(p: &mut Painter, text: &str, good: Style, bad: Style) {
    let (mut apostrophe, mut space, mut clean) = (false, false, true);
    for &byte in text.as_bytes() {
        match byte {
            b'\'' => apostrophe = true,
            b' ' => space = true,
            0..=0x1f | 0x7f => clean = false,
            _ => {}
        }
    }
    let quote: &[u8] = if apostrophe {
        b"\""
    } else if space {
        b"'"
    } else {
        b""
    };
    if !quote.is_empty() {
        p.push(&good, quote);
    }
    if clean {
        p.push(&good, text.as_bytes());
    } else {
        let mut buf = [0u8; 4];
        for c in text.chars() {
            if printable(c) {
                p.push(&good, c.encode_utf8(&mut buf).as_bytes());
            } else {
                let escaped: String = c.escape_default().collect();
                p.push(&bad, escaped.as_bytes());
            }
        }
    }
    if !quote.is_empty() {
        p.push(&good, quote);
    }
}

fn file_name_of(path: &Path) -> String {
    match path.components().next_back() {
        Some(last) => last.as_os_str().to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

fn ext_lower<'b>(name: &str, buf: &'b mut [u8; 32]) -> Option<&'b str> {
    let ext = &name[name.rfind('.')? + 1..];
    if ext.len() > buf.len() {
        return Some("\0");
    }
    let out = &mut buf[..ext.len()];
    out.copy_from_slice(ext.as_bytes());
    out.make_ascii_lowercase();
    std::str::from_utf8(out).ok()
}

fn sources_of(ext: &str) -> &'static [&'static str] {
    match ext {
        "css" => &["sass", "scss", "styl", "less"],
        "mjs" => &["mts"],
        "cjs" => &["cts"],
        "js" => &["coffee", "ts"],
        "aux" | "bbl" | "bcf" | "blg" | "fdb_latexmk" | "fls" | "headfootlength" | "lof"
        | "log" | "lot" | "out" | "toc" | "xdv" => &["tex"],
        _ => &[],
    }
}

fn mount_points() -> HashSet<PathBuf> {
    let Ok(text) = std::fs::read("/proc/self/mounts") else {
        return HashSet::new();
    };
    text.split(|&b| b == b'\n')
        .filter_map(|line| line.split(|&b| b == b' ').nth(1))
        .map(|field| {
            let mut out = Vec::with_capacity(field.len());
            let mut at = 0;
            while at < field.len() {
                let octal = field.get(at + 1..at + 4).filter(|digits| {
                    field[at] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d))
                });
                match octal {
                    Some(digits) => {
                        out.push(digits.iter().fold(0u8, |acc, d| acc * 8 + (d - b'0')));
                        at += 4;
                    }
                    None => {
                        out.push(field[at]);
                        at += 1;
                    }
                }
            }
            PathBuf::from(std::ffi::OsStr::from_bytes(&out))
        })
        .collect()
}

fn has_xattrs(path: &Path, scratch: &mut Vec<u8>) -> bool {
    scratch.clear();
    scratch.extend_from_slice(path.as_os_str().as_bytes());
    if scratch.contains(&0) {
        return false;
    }
    scratch.push(0);
    let len = unsafe { nix::libc::llistxattr(scratch.as_ptr().cast(), std::ptr::null_mut(), 0) };
    len > 0
}

fn size_cell(row: &Row) -> SizeCell {
    let meta = &row.entry.meta;
    match row.kind {
        Kind::File => {
            let size = meta.len();
            if size < 1000 {
                let mut number = Text::default();
                if size >= 100 {
                    number.push_byte(b'0' + (size / 100) as u8);
                }
                if size >= 10 {
                    number.push_byte(b'0' + (size / 10 % 10) as u8);
                }
                number.push_byte(b'0' + (size % 10) as u8);
                return SizeCell::Number(number);
            }
            let mut amount = size as f64;
            let mut slot = 0;
            while amount >= 1000.0 && slot < 8 {
                amount /= 1000.0;
                slot += 1;
            }
            let mut number = Text::default();
            if amount < 10.0 {
                let _ = write!(number, "{amount:.1}");
            } else {
                let _ = write!(number, "{}", amount.round() as u64);
            }
            SizeCell::Prefixed {
                number,
                unit: b"kMGTPEZY"[slot - 1],
                slot,
            }
        }
        Kind::Char | Kind::Block => {
            let dev = meta.rdev();
            SizeCell::Device {
                major: (((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0xfff)) as u32,
                minor: (((dev >> 12) & 0xffff_ff00) | (dev & 0xff)) as u32,
            }
        }
        _ => SizeCell::Dash,
    }
}

type PermissionCache = FxHashMap<(u8, u32, bool), (Vec<u8>, Option<Style>)>;

#[derive(Clone, Copy)]
struct Widths {
    perm: usize,
    size: usize,
    user: usize,
    date: usize,
}

impl Default for Widths {
    fn default() -> Widths {
        Widths {
            perm: 11,
            size: 4,
            user: 4,
            date: 13,
        }
    }
}

impl Widths {
    fn fit(&mut self, row: &Row, cell: &Cells, users: &[UserName]) {
        self.perm = self.perm.max(10 + usize::from(row.xattr));
        self.size = self.size.max(match cell.size {
            SizeCell::Dash => 1,
            SizeCell::Number(number) => number.width(),
            SizeCell::Prefixed { number, .. } => number.width() + 1,
            SizeCell::Device { major, minor } => {
                major.to_string().len() + 1 + minor.to_string().len()
            }
        });
        self.user = self.user.max(users[cell.user].width);
        self.date = self.date.max(cell.date.width());
    }

    fn max(self, other: Widths) -> Widths {
        Widths {
            perm: self.perm.max(other.perm),
            size: self.size.max(other.size),
            user: self.user.max(other.user),
            date: self.date.max(other.date),
        }
    }
}

fn cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8)
    })
}

fn chunk_ranges(len: usize, min_chunk: usize) -> Vec<Range<usize>> {
    let parts = (len / min_chunk).clamp(1, cores());
    let size = len.div_ceil(parts).max(1);
    (0..len)
        .step_by(size)
        .map(|at| at..(at + size).min(len))
        .collect()
}

fn parallel_ranges<R: Send>(
    len: usize,
    min_chunk: usize,
    work: impl Fn(Range<usize>) -> R + Sync,
) -> Vec<R> {
    let mut ranges = chunk_ranges(len, min_chunk);
    if ranges.len() < 2 {
        return ranges.pop().map(work).into_iter().collect();
    }
    let work = &work;
    std::thread::scope(|scope| {
        let handles: Vec<_> = ranges
            .into_iter()
            .map(|range| scope.spawn(move || work(range)))
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    })
}

fn sort_parallel<T: Copy + Send + Sync>(
    mut items: Vec<T>,
    compare: &(impl Fn(&T, &T) -> Ordering + Sync),
) -> Vec<T> {
    let ranges = chunk_ranges(items.len(), 2048);
    if ranges.len() < 2 {
        items.sort_by(compare);
        return items;
    }
    let size = ranges[0].len();
    std::thread::scope(|scope| {
        for part in items.chunks_mut(size) {
            scope.spawn(move || part.sort_by(compare));
        }
    });
    let mut runs: Vec<Vec<T>> = items.chunks(size).map(<[T]>::to_vec).collect();
    while runs.len() > 1 {
        let mut next = Vec::with_capacity(runs.len().div_ceil(2));
        let mut pending = runs.into_iter();
        while let Some(left) = pending.next() {
            match pending.next() {
                Some(right) => next.push(merge(&left, &right, compare)),
                None => next.push(left),
            }
        }
        runs = next;
    }
    runs.pop().unwrap_or_default()
}

fn merge<T: Copy>(left: &[T], right: &[T], compare: &impl Fn(&T, &T) -> Ordering) -> Vec<T> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        if compare(&right[j], &left[i]) == Ordering::Less {
            out.push(right[j]);
            j += 1;
        } else {
            out.push(left[i]);
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    out
}

impl<'s> Ctx<'s> {
    fn new(env: &Environment, settings: &'s Settings) -> Ctx<'s> {
        let color = !env.no_color;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let offset = local_offset(now);
        let icons =
            (env.columns.is_some() || terminal_width().is_some()).then_some(env.icon_spacing);
        Ctx {
            theme: Theme::new(color, env.ls_colors.as_deref(), env.eza_colors.as_deref()),
            icons,
            settings,
            offset,
            year: civil_from_days((now + offset).div_euclid(86_400)).0,
            me: nix::unistd::getuid().as_raw(),
            users: Vec::new(),
            user_index: HashMap::new(),
            last_user: (u32::MAX, 0),
            mounts: OnceLock::new(),
        }
    }

    fn is_mount(&self, path: &Path) -> bool {
        self.mounts.get_or_init(mount_points).contains(path)
    }

    fn make_row<'e>(&mut self, entry: &'e Entry, canonical_dir: Option<&Path>) -> Row<'e> {
        let kind = kind_of(&entry.meta);
        let link = if kind == Kind::Link {
            self.read_link(entry)
        } else {
            Link::None
        };
        let dirlike = kind == Kind::Dir || matches!(&link, Link::Ok(t) if t.meta.is_dir());
        let mount = kind == Kind::Dir
            && match canonical_dir {
                Some(dir) => self.is_mount(&dir.join(&entry.name)),
                None => std::fs::canonicalize(&entry.path).is_ok_and(|p| self.is_mount(&p)),
            };
        Row {
            entry,
            kind,
            link,
            dirlike,
            mount,
            xattr: entry.xattr,
        }
    }

    fn read_link(&self, entry: &Entry) -> Link {
        let Ok(raw) = std::fs::read_link(&entry.path) else {
            return Link::Unreadable;
        };
        let absolute = if raw.is_absolute() {
            raw.clone()
        } else {
            match entry.path.parent() {
                Some(parent) => parent.join(&raw),
                None => entry.path.join(&raw),
            }
        };
        match std::fs::metadata(&absolute) {
            Ok(meta) => {
                let mount = meta.is_dir() && self.is_mount(&absolute);
                Link::Ok(Box::new(Target { raw, meta, mount }))
            }
            Err(_) => Link::Broken(raw),
        }
    }

    fn user(&mut self, uid: u32) -> usize {
        if self.last_user.0 == uid && !self.users.is_empty() {
            return self.last_user.1;
        }
        if let Some(&at) = self.user_index.get(&uid) {
            self.last_user = (uid, at);
            return at;
        }
        let name = match nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid)) {
            Ok(Some(user)) => user.name,
            _ => uid.to_string(),
        };
        let style = if uid == self.me {
            self.theme.user_you
        } else if uid == 0 {
            self.theme.user_root
        } else {
            self.theme.user_other
        };
        self.users.push(UserName {
            width: name.chars().count(),
            name,
            style,
        });
        let at = self.users.len() - 1;
        self.user_index.insert(uid, at);
        self.last_user = (uid, at);
        at
    }

    fn date(&self, meta: &Metadata, civil: &mut (i64, (i64, i64, i64))) -> Text {
        let mut text = Text::default();
        let secs = meta.mtime();
        if secs < 0 {
            text.push_byte(b'-');
            return text;
        }
        let local = secs + self.offset;
        let days = local.div_euclid(86_400);
        if civil.0 != days {
            *civil = (days, civil_from_days(days));
        }
        let (year, month, day) = civil.1;
        text.push_two_digits(day, b' ');
        text.push_byte(b' ');
        text.push_str(MONTHS[(month - 1) as usize]);
        if year == self.year {
            let rem = local.rem_euclid(86_400);
            text.push_byte(b' ');
            text.push_two_digits(rem / 3600, b'0');
            text.push_byte(b':');
            text.push_two_digits(rem % 3600 / 60, b'0');
        } else {
            text.push_str("  ");
            let _ = write!(text, "{year:04}");
        }
        text
    }

    fn row_style(&self, row: &Row, siblings: &Siblings, ext: Option<&str>) -> Style {
        let theme = &self.theme;
        let name = row.entry.name.as_str();
        if row.mount {
            return theme.mount_point;
        }
        match row.kind {
            Kind::Dir => theme.directory,
            Kind::File if row.entry.meta.mode() & 0o100 != 0 => theme.executable,
            Kind::Link => theme.symlink,
            Kind::Pipe => theme.pipe,
            Kind::Block => theme.block_device,
            Kind::Char => theme.char_device,
            Kind::Socket => theme.socket,
            Kind::Special => theme.special,
            Kind::File => self.file_style(name, ext, siblings),
        }
    }

    fn file_style(&self, name: &str, ext: Option<&str>, siblings: &Siblings) -> Style {
        self.theme.file_style(name, ext, || {
            ext.is_some_and(|ext| {
                sources_of(ext).iter().any(|source| {
                    siblings.contains(&Path::new(name).with_extension(source).to_string_lossy())
                })
            })
        })
    }

    fn target_style(&self, target: &Target) -> Style {
        let theme = &self.theme;
        let name = file_name_of(&target.raw);
        let meta = &target.meta;
        if target.mount {
            theme.mount_point
        } else if meta.is_dir() {
            theme.directory
        } else if meta.is_file() && meta.mode() & 0o100 != 0 {
            theme.executable
        } else {
            match kind_of(meta) {
                Kind::Pipe => theme.pipe,
                Kind::Block => theme.block_device,
                Kind::Char => theme.char_device,
                Kind::Socket => theme.socket,
                Kind::File => {
                    let mut buf = [0u8; 32];
                    theme.file_style(&name, ext_lower(&name, &mut buf), || false)
                }
                _ => theme.special,
            }
        }
    }

    fn icon_for(&self, row: &Row, ext: Option<&str>) -> char {
        let name = row.entry.name.as_str();
        if row.dirlike {
            if let Some(glyph) = directory_icon(name) {
                glyph
            } else if row.kind == Kind::Dir && is_empty_dir(row.entry) {
                FOLDER_OPEN
            } else {
                FOLDER
            }
        } else if let Some(glyph) = filename_icon(name) {
            glyph
        } else if let Some(ext) = ext {
            extension_icon(ext).unwrap_or(FILE)
        } else {
            FILE_UNKNOWN
        }
    }

    fn parent_bits(&self, p: &mut Painter, parent: &Path) {
        let theme = &self.theme;
        let count = parent.components().count();
        if count == 1 && parent.has_root() {
            p.push(&theme.symlink_path, b"/");
        } else if count >= 1 {
            escape(
                p,
                &parent.to_string_lossy(),
                theme.symlink_path,
                theme.control_char,
            );
            p.push(&theme.symlink_path, b"/");
        }
    }

    fn paint_name(&self, p: &mut Painter, row: &Row, siblings: &Siblings) {
        let theme = &self.theme;
        let mut ext_buf = [0u8; 32];
        let ext = ext_lower(&row.entry.name, &mut ext_buf);
        let style = self.row_style(row, siblings, ext);
        if let Some(spacing) = self.icons {
            let icon_style = style.icon_style();
            let mut buf = [0u8; 4];
            p.push(
                &icon_style,
                self.icon_for(row, ext).encode_utf8(&mut buf).as_bytes(),
            );
            p.pad(&icon_style, spacing);
        }
        if let Some(typed) = &row.entry.typed
            && let Some(parent) = Path::new(typed).parent()
        {
            self.parent_bits(p, parent);
        }
        escape(p, &row.entry.name, style, theme.control_char);
        match &row.link {
            Link::Ok(target) => {
                p.push(&Style::NONE, b" ");
                p.push(&theme.punctuation, b"->");
                p.push(&Style::NONE, b" ");
                if let Some(parent) = target.raw.parent() {
                    self.parent_bits(p, parent);
                }
                escape(
                    p,
                    &file_name_of(&target.raw),
                    self.target_style(target),
                    theme.control_char,
                );
            }
            Link::Broken(raw) => {
                p.push(&Style::NONE, b" ");
                p.push(&theme.broken_symlink, b"->");
                p.push(&Style::NONE, b" ");
                escape(
                    p,
                    &raw.display().to_string(),
                    theme.broken_symlink.overlay(theme.broken_path_overlay),
                    theme.control_char.overlay(theme.broken_path_overlay),
                );
            }
            Link::None | Link::Unreadable => {}
        }
    }

    fn sorted<'a, 'e>(&self, rows: Vec<&'a Row<'e>>) -> Vec<&'a Row<'e>> {
        let sort = self.settings.sort;
        let compare = |a: &&Row, b: &&Row| {
            let by_name = || match (&a.entry.key, &b.entry.key) {
                (Some(l), Some(r)) => l.cmp(r),
                _ => compare_ignore_case(&a.entry.name, &b.entry.name),
            };
            match sort {
                Sort::Name => by_name(),
                Sort::Newest => {
                    let key = |r: &Row| (r.entry.meta.mtime(), r.entry.meta.mtime_nsec());
                    key(b).cmp(&key(a)).then_with(by_name)
                }
                Sort::Largest => b
                    .entry
                    .meta
                    .len()
                    .cmp(&a.entry.meta.len())
                    .then_with(by_name),
            }
        };
        let mut rows = sort_parallel(rows, &compare);
        if self.settings.reverse {
            rows.reverse();
        }
        let (mut dirs, files): (Vec<_>, Vec<_>) = rows.into_iter().partition(|row| row.dirlike);
        dirs.extend(files);
        dirs
    }

    fn table(&mut self, p: &mut Painter, dir: Option<&Path>, entries: &[Entry]) {
        if entries.is_empty() {
            return;
        }
        let canonical_dir = dir.and_then(|dir| std::fs::canonicalize(dir).ok());
        let rows: Vec<Row> = entries
            .iter()
            .map(|entry| self.make_row(entry, canonical_dir.as_deref()))
            .collect();
        let mut rows: Vec<&Row> = self.sorted(rows.iter().collect());
        if dir.is_some() && self.settings.reverse {
            rows = self.sorted(rows);
        }
        let siblings = Siblings {
            entries,
            names: OnceLock::new(),
            enabled: dir.is_some(),
        };
        if self.settings.lines {
            let parts = parallel_ranges(rows.len(), 2048, |range| {
                let mut p = Painter::with_capacity(range.len() * 64);
                for row in &rows[range] {
                    self.paint_name(&mut p, row, &siblings);
                    p.end_line();
                }
                p.out
            });
            parts.iter().for_each(|part| p.raw(part));
        } else {
            self.details(p, &rows, &siblings);
        }
    }

    fn details(&mut self, p: &mut Painter, rows: &[&Row], siblings: &Siblings) {
        let uids: Vec<usize> = rows
            .iter()
            .map(|row| self.user(row.entry.meta.uid()))
            .collect();
        let this = &*self;
        let measured = parallel_ranges(rows.len(), 2048, |range| {
            let mut widths = Widths::default();
            let mut day = (i64::MIN, (0, 0, 0));
            let mut cells = Vec::with_capacity(range.len());
            for at in range {
                let row = rows[at];
                let cell = Cells {
                    size: size_cell(row),
                    user: uids[at],
                    date: this.date(&row.entry.meta, &mut day),
                };
                widths.fit(row, &cell, &this.users);
                cells.push(cell);
            }
            (cells, widths)
        });
        let mut widths = Widths::default();
        let mut cells = Vec::with_capacity(rows.len());
        for (part, fitted) in measured {
            widths = widths.max(fitted);
            cells.extend(part);
        }

        let header = this.theme.header;
        p.push(&header, b"Permissions");
        p.spaces(widths.perm - 11 + 1);
        p.spaces(widths.size - 4);
        p.push(&header, b"Size");
        p.spaces(1);
        p.push(&header, b"User");
        p.spaces(widths.user - 4 + 1);
        p.push(&header, b"Date Modified");
        p.spaces(widths.date - 13 + 1);
        p.push(&header, b"Name");
        p.end_line();

        let parts = parallel_ranges(rows.len(), 2048, |range| {
            let mut p = Painter::with_capacity(range.len() * 160);
            let mut cache = PermissionCache::default();
            for at in range {
                this.emit(&mut p, rows[at], &cells[at], &widths, siblings, &mut cache);
            }
            p.out
        });
        parts.iter().for_each(|part| p.raw(part));
    }

    fn emit(
        &self,
        p: &mut Painter,
        row: &Row,
        cell: &Cells,
        widths: &Widths,
        siblings: &Siblings,
        cache: &mut PermissionCache,
    ) {
        let key = (row.kind as u8, row.entry.meta.mode() & 0o7777, row.xattr);
        if let Some((bytes, last)) = cache.get(&key) {
            p.replay(bytes, *last);
        } else {
            let start = p.out.len();
            self.permissions(p, row);
            cache.insert(key, (p.out[start..].to_vec(), p.last()));
        }
        p.spaces(widths.perm - 10 - usize::from(row.xattr) + 1);
        self.size(p, cell.size, widths.size);
        p.spaces(1);
        let user = &self.users[cell.user];
        p.push(&user.style, user.name.as_bytes());
        p.spaces(widths.user - user.width + 1);
        p.push(&self.theme.date, cell.date.bytes());
        p.spaces(widths.date - cell.date.width() + 1);
        self.paint_name(p, row, siblings);
        p.end_line();
    }

    fn permissions(&self, p: &mut Painter, row: &Row) {
        let theme = &self.theme;
        let mode = row.entry.meta.mode();
        let dash = theme.punctuation;
        let (kind_char, kind_style) = match row.kind {
            Kind::File => (b'.', theme.normal),
            Kind::Dir => (b'd', theme.directory),
            Kind::Pipe => (b'|', theme.pipe),
            Kind::Link => (b'l', theme.symlink),
            Kind::Block => (b'b', theme.block_device),
            Kind::Char => (b'c', theme.char_device),
            Kind::Socket => (b's', theme.socket),
            Kind::Special => (b'?', theme.special),
        };
        p.push(&kind_style, &[kind_char]);
        let bit = |p: &mut Painter, mask: u32, ch: u8, style: Style| {
            if mode & mask != 0 {
                p.push(&style, &[ch]);
            } else {
                p.push(&dash, b"-");
            }
        };
        bit(p, 0o400, b'r', theme.user_read);
        bit(p, 0o200, b'w', theme.user_write);
        let regular = row.kind == Kind::File;
        match (mode & 0o100 != 0, mode & 0o4000 != 0, regular) {
            (false, false, _) => p.push(&dash, b"-"),
            (true, false, false) => p.push(&theme.user_execute_other, b"x"),
            (true, false, true) => p.push(&theme.user_execute_file, b"x"),
            (false, true, _) => p.push(&theme.special_other, b"S"),
            (true, true, false) => p.push(&theme.special_other, b"s"),
            (true, true, true) => p.push(&theme.special_user_file, b"s"),
        }
        bit(p, 0o040, b'r', theme.group_read);
        bit(p, 0o020, b'w', theme.group_write);
        match (mode & 0o010 != 0, mode & 0o2000 != 0) {
            (false, false) => p.push(&dash, b"-"),
            (true, false) => p.push(&theme.group_execute, b"x"),
            (false, true) => p.push(&theme.special_other, b"S"),
            (true, true) => p.push(&theme.special_other, b"s"),
        }
        bit(p, 0o004, b'r', theme.other_read);
        bit(p, 0o002, b'w', theme.other_write);
        match (mode & 0o001 != 0, mode & 0o1000 != 0) {
            (false, false) => p.push(&dash, b"-"),
            (true, false) => p.push(&theme.other_execute, b"x"),
            (false, true) => p.push(&theme.special_other, b"T"),
            (true, true) => p.push(&theme.special_other, b"t"),
        }
        if row.xattr {
            p.push(&theme.attribute, b"@");
        }
    }

    fn size(&self, p: &mut Painter, cell: SizeCell, width: usize) {
        let theme = &self.theme;
        match cell {
            SizeCell::Dash => {
                p.spaces(width - 1);
                p.push(&theme.punctuation, b"-");
            }
            SizeCell::Number(number) => {
                p.spaces(width - number.width());
                p.push(&theme.size_number(None), number.bytes());
            }
            SizeCell::Prefixed { number, unit, slot } => {
                p.spaces(width - number.width() - 1);
                p.push(&theme.size_number(Some(slot)), number.bytes());
                p.push(&theme.size_number(Some(slot)), &[unit]);
            }
            SizeCell::Device { major, minor } => {
                let (major, minor) = (major.to_string(), minor.to_string());
                p.spaces(width - major.len() - 1 - minor.len());
                p.push(&theme.major, major.as_bytes());
                p.push(&theme.punctuation, b",");
                p.push(&theme.minor, minor.as_bytes());
            }
        }
    }
}

fn is_empty_dir(entry: &Entry) -> bool {
    if entry.meta.nlink() > 2 {
        return false;
    }
    match std::fs::read_dir(&entry.path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => false,
    }
}

pub(crate) fn render(layout: Layout, env: &Environment, settings: &Settings) -> Vec<u8> {
    let mut ctx = Ctx::new(env, settings);
    let rows = layout.files.len()
        + layout
            .folders
            .iter()
            .map(|f| f.entries.len())
            .sum::<usize>();
    let mut p = Painter::with_capacity(rows * 160 + 256);
    let no_files = layout.files.is_empty();
    ctx.table(&mut p, None, &layout.files);
    let only_folder = layout.folders.len() == 1 && no_files;
    let mut first = no_files;
    for folder in &layout.folders {
        if first {
            first = false;
        } else {
            p.raw(b"\n");
        }
        if !only_folder {
            escape(&mut p, &folder.heading, Style::NONE, Style::NONE);
            p.push(&Style::NONE, b":");
            p.end_line();
        }
        ctx.table(&mut p, Some(&folder.path), &folder.entries);
    }
    p.out
}
