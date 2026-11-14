//! The engine behind `head` and `tail`: select the first or last N lines or bytes of
//! files, of standard input, or of an upstream table.
//!
//! Raw output is byte-exact. A regular file is never read past what the selection needs:
//! `head` stops after N lines, `tail` scans backwards from the end in chunks, as uutils
//! `tail/src/chunks.rs` (`ReverseChunks`) does. Hashline output also needs the line
//! numbers of a tail, so it counts the newlines before the window without keeping them.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom};

use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::{HashlineRows, push_hashline_rows, resolve_path, text_lines, usage_error};
use super::fs_ops::{describe, fail};
use crate::args::Args;
use crate::builtin::{Flag, Signature};

const CHUNK: usize = 64 * 1024;
const DEFAULT_LINES: u64 = 10;

/// Flags shared by `head` and `tail`.
pub(super) const FLAGS: [Flag; 5] = [
    Flag::valued('n', "lines", "number of lines").numeric(),
    Flag::valued('c', "bytes", "number of bytes; the output is raw"),
    Flag::switch('r', "raw", "byte-exact output instead of Hashline rows"),
    Flag::switch('q', "quiet", "never print file name headers"),
    Flag::switch('v', "verbose", "always print file name headers"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    Head,
    Tail,
}

#[derive(Clone, Copy)]
enum Unit {
    Lines,
    Bytes,
}

#[derive(Clone, Copy)]
enum Window {
    First(u64),
    Last(u64),
    /// `tail -n +N`: everything from the 1-based position N on.
    From(u64),
}

fn parse_window(
    sig: &Signature,
    end: End,
    raw: &str,
    what: &str,
) -> Result<Window, StructuredDiagnostic> {
    let invalid = || {
        usage_error(
            format!("{}: invalid number of {what}: '{raw}'", sig.name),
            &format!("{} [-n N | -c N] [FILE]...", sig.name),
        )
    };
    match (end, raw.strip_prefix('+')) {
        (End::Tail, Some(from)) => from.parse().map(Window::From).map_err(|_| invalid()),
        (End::Head, _) => raw.parse().map(Window::First).map_err(|_| invalid()),
        (End::Tail, None) => raw.parse().map(Window::Last).map_err(|_| invalid()),
    }
}

pub(super) fn run(
    session: &mut Session,
    sig: &'static Signature,
    end: End,
    raw_args: &[String],
    input: Value,
) -> Result<Value, StructuredDiagnostic> {
    let args = Args::parse(sig, raw_args)?;
    let (unit, what) = match args.last_of(&["n", "c"]) {
        Some("c") => (Unit::Bytes, "bytes"),
        _ => (Unit::Lines, "lines"),
    };
    let key = if matches!(unit, Unit::Bytes) {
        "c"
    } else {
        "n"
    };
    let default = DEFAULT_LINES.to_string();
    let window = parse_window(sig, end, args.value(key).unwrap_or(&default), what)?;
    let raw = args.has("raw") || matches!(unit, Unit::Bytes);

    if args.operands.is_empty() {
        return from_input(input, unit, window);
    }

    let named = args.operands.len() > 1;
    let headers = args.has("verbose") || (named && !args.has("quiet"));
    let mut input = Some(input);
    let mut out = Vec::new();
    let mut rows = HashlineRows::new();
    let mut printed = 0usize;
    for operand in &args.operands {
        let (label, mut source) = if *operand == "-" {
            let bytes = input.take().map(Value::into_raw_bytes).unwrap_or_default();
            ("standard input", Source::Memory(Cursor::new(bytes)))
        } else {
            match open(session, sig, operand) {
                Some(source) => (*operand, source),
                None => continue,
            }
        };
        let (start, bytes) = match source.select(unit, window) {
            Ok(selected) => selected,
            Err(e) => {
                fail(
                    session,
                    "E_READ",
                    format!("{}: error reading '{label}': {}", sig.name, describe(&e)),
                );
                continue;
            }
        };
        if raw {
            if headers {
                if printed > 0 {
                    out.push(b'\n');
                }
                out.extend_from_slice(format!("==> {label} <==\n").as_bytes());
            }
            out.extend_from_slice(&bytes);
        } else {
            let first = match window {
                Window::First(_) => 1,
                Window::From(n) => n.max(1),
                Window::Last(_) => match source.newlines_before(start) {
                    Ok(count) => count + 1,
                    Err(e) => {
                        fail(
                            session,
                            "E_READ",
                            format!("{}: error reading '{label}': {}", sig.name, describe(&e)),
                        );
                        continue;
                    }
                },
            };
            push_hashline_rows(&mut rows, label, first, text_lines(&bytes));
        }
        printed += 1;
    }
    Ok(if raw {
        Value::from_bytes(out)
    } else {
        Value::from_batch(rows.finish()?)
    })
}

/// No operands: the upstream table keeps its rows, anything else is treated as bytes.
fn from_input(input: Value, unit: Unit, window: Window) -> Result<Value, StructuredDiagnostic> {
    match input {
        Value::Empty => Ok(Value::Empty),
        Value::Batch(mut batch) if matches!(unit, Unit::Lines) => {
            match window {
                Window::First(n) => batch.truncate_rows(to_usize(n)),
                Window::Last(n) => batch.tail_rows(to_usize(n)),
                Window::From(n) => {
                    let skipped = to_usize(n.saturating_sub(1));
                    batch.tail_rows(batch.nrows().saturating_sub(skipped));
                }
            }
            Ok(Value::from_batch(batch))
        }
        other => {
            let mut source = Source::Memory(Cursor::new(other.into_raw_bytes()));
            let (_, bytes) = source
                .select(unit, window)
                .map_err(|e| StructuredDiagnostic::from_io("E_READ", &e, None))?;
            Ok(Value::from_bytes(bytes))
        }
    }
}

fn to_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

fn open(session: &mut Session, sig: &Signature, operand: &str) -> Option<Source> {
    let path = resolve_path(session, operand);
    let meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) => {
            fail(
                session,
                "E_READ",
                format!(
                    "{}: cannot open '{operand}' for reading: {}",
                    sig.name,
                    describe(&e)
                ),
            );
            return None;
        }
    };
    if meta.is_dir() {
        fail(
            session,
            "E_READ",
            format!("{}: error reading '{operand}': Is a directory", sig.name),
        );
        return None;
    }
    let opened = File::open(&path).and_then(|mut file| {
        if meta.is_file() {
            Ok(Source::Seekable(file, meta.len()))
        } else {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(Source::Memory(Cursor::new(bytes)))
        }
    });
    match opened {
        Ok(source) => Some(source),
        Err(e) => {
            fail(
                session,
                "E_READ",
                format!(
                    "{}: cannot open '{operand}' for reading: {}",
                    sig.name,
                    describe(&e)
                ),
            );
            None
        }
    }
}

enum Source {
    Seekable(File, u64),
    Memory(Cursor<Vec<u8>>),
}

impl Source {
    fn len(&self) -> u64 {
        match self {
            Source::Seekable(_, len) => *len,
            Source::Memory(cursor) => cursor.get_ref().len() as u64,
        }
    }

    fn with<T>(
        &mut self,
        f: impl FnOnce(&mut dyn ReadSeek, u64) -> io::Result<T>,
    ) -> io::Result<T> {
        let len = self.len();
        match self {
            Source::Seekable(file, _) => f(file, len),
            Source::Memory(cursor) => f(cursor, len),
        }
    }

    /// The selected bytes and the offset they start at.
    fn select(&mut self, unit: Unit, window: Window) -> io::Result<(u64, Vec<u8>)> {
        self.with(|s, len| select(s, len, unit, window))
    }

    fn newlines_before(&mut self, offset: u64) -> io::Result<u64> {
        self.with(|s, _| count_newlines(s, offset))
    }
}

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

fn select(
    s: &mut dyn ReadSeek,
    len: u64,
    unit: Unit,
    window: Window,
) -> io::Result<(u64, Vec<u8>)> {
    s.seek(SeekFrom::Start(0))?;
    let start = match (unit, window) {
        (Unit::Bytes, Window::First(n)) => {
            let mut bytes = Vec::new();
            s.take(n).read_to_end(&mut bytes)?;
            return Ok((0, bytes));
        }
        (Unit::Lines, Window::First(n)) => {
            let mut reader = BufReader::new(s);
            let mut bytes = Vec::new();
            for _ in 0..n {
                if reader.read_until(b'\n', &mut bytes)? == 0 {
                    break;
                }
            }
            return Ok((0, bytes));
        }
        (Unit::Bytes, Window::Last(n)) => len.saturating_sub(n),
        (Unit::Bytes, Window::From(n)) => n.saturating_sub(1).min(len),
        (Unit::Lines, Window::Last(n)) => last_lines_start(s, len, n)?,
        (Unit::Lines, Window::From(n)) => skip_lines(s, n.saturating_sub(1))?,
    };
    s.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    s.read_to_end(&mut bytes)?;
    Ok((start, bytes))
}

/// Offset of the first byte of the last `n` lines, found by reading chunks from the end.
fn last_lines_start(s: &mut dyn ReadSeek, len: u64, n: u64) -> io::Result<u64> {
    if n == 0 {
        return Ok(len);
    }
    let mut buf = vec![0u8; CHUNK];
    let mut pos = len;
    let mut seen = 0u64;
    let mut at_end = true;
    while pos > 0 {
        let take = pos.min(CHUNK as u64) as usize;
        pos -= take as u64;
        s.seek(SeekFrom::Start(pos))?;
        s.read_exact(&mut buf[..take])?;
        let mut chunk = &buf[..take];
        if at_end {
            // The newline that ends the last line separates nothing.
            if chunk.last() == Some(&b'\n') {
                chunk = &chunk[..take - 1];
            }
            at_end = false;
        }
        let mut limit = chunk.len();
        while let Some(at) = memchr::memrchr(b'\n', &chunk[..limit]) {
            seen += 1;
            if seen == n {
                return Ok(pos + at as u64 + 1);
            }
            limit = at;
        }
    }
    Ok(0)
}

/// Offset just past the first `lines` lines.
fn skip_lines(s: &mut dyn ReadSeek, lines: u64) -> io::Result<u64> {
    let mut reader = BufReader::new(s);
    let mut offset = 0u64;
    let mut scratch = Vec::new();
    for _ in 0..lines {
        scratch.clear();
        let read = reader.read_until(b'\n', &mut scratch)?;
        if read == 0 {
            break;
        }
        offset += read as u64;
    }
    Ok(offset)
}

fn count_newlines(s: &mut dyn ReadSeek, before: u64) -> io::Result<u64> {
    s.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::with_capacity(CHUNK, s.take(before));
    let mut count = 0u64;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(count);
        }
        count += memchr::memchr_iter(b'\n', chunk).count() as u64;
        let used = chunk.len();
        reader.consume(used);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(bytes: &[u8], unit: Unit, window: Window) -> (u64, Vec<u8>) {
        let mut source = Source::Memory(Cursor::new(bytes.to_vec()));
        source.select(unit, window).unwrap()
    }

    #[test]
    fn last_lines_ignore_the_final_newline_and_keep_a_missing_one() {
        let (start, bytes) = pick(b"a\nb\nc\n", Unit::Lines, Window::Last(2));
        assert_eq!((start, bytes.as_slice()), (2, &b"b\nc\n"[..]));
        let (_, bytes) = pick(b"a\nb\nc", Unit::Lines, Window::Last(2));
        assert_eq!(bytes, b"b\nc");
        let (_, bytes) = pick(b"a\nb\nc\n", Unit::Lines, Window::Last(10));
        assert_eq!(bytes, b"a\nb\nc\n");
        let (_, bytes) = pick(b"a\n\n\n", Unit::Lines, Window::Last(2));
        assert_eq!(bytes, b"\n\n");
        let (_, bytes) = pick(b"", Unit::Lines, Window::Last(3));
        assert!(bytes.is_empty());
        let (_, bytes) = pick(b"a\nb\n", Unit::Lines, Window::Last(0));
        assert!(bytes.is_empty());
    }

    #[test]
    fn last_lines_cross_chunk_boundaries() {
        let mut text = Vec::new();
        for i in 0..40_000 {
            text.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        let (_, bytes) = pick(&text, Unit::Lines, Window::Last(3));
        assert_eq!(bytes, b"line 39997\r\nline 39998\r\nline 39999\r\n");
    }

    #[test]
    fn first_lines_and_bytes_are_exact() {
        let (_, bytes) = pick(b"a\r\nb\r\nc", Unit::Lines, Window::First(2));
        assert_eq!(bytes, b"a\r\nb\r\n");
        let (_, bytes) = pick(b"a\r\nb\r\nc", Unit::Lines, Window::First(9));
        assert_eq!(bytes, b"a\r\nb\r\nc");
        let (_, bytes) = pick(b"abcdef", Unit::Bytes, Window::First(4));
        assert_eq!(bytes, b"abcd");
        let (_, bytes) = pick(b"abcdef", Unit::Bytes, Window::Last(4));
        assert_eq!(bytes, b"cdef");
    }

    #[test]
    fn from_windows_start_at_the_given_position() {
        let (_, bytes) = pick(b"h\n1\n2\n", Unit::Lines, Window::From(2));
        assert_eq!(bytes, b"1\n2\n");
        let (_, bytes) = pick(b"h\n1\n2\n", Unit::Lines, Window::From(1));
        assert_eq!(bytes, b"h\n1\n2\n");
        let (_, bytes) = pick(b"abcdef", Unit::Bytes, Window::From(3));
        assert_eq!(bytes, b"cdef");
        let (_, bytes) = pick(b"a\n", Unit::Lines, Window::From(9));
        assert!(bytes.is_empty());
    }

    #[test]
    fn line_numbers_of_a_tail_count_the_skipped_newlines() {
        let mut source = Source::Memory(Cursor::new(b"a\nb\nc\nd\n".to_vec()));
        let (start, _) = source.select(Unit::Lines, Window::Last(2)).unwrap();
        assert_eq!(source.newlines_before(start).unwrap(), 2);
    }
}
