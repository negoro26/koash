use std::io::{self, Write};
use std::path::Path;
use std::sync::Once;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::queue;
use crossterm::terminal::{self, Clear, ClearType};
use shell_value::StructuredDiagnostic;

use crate::complete::complete_line;
use crate::highlight::highlight_line;
use crate::history::History;

static PANIC_HOOK_INIT: Once = Once::new();

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Result<Self, StructuredDiagnostic> {
        PANIC_HOOK_INIT.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let _ = terminal::disable_raw_mode();
                prev(info);
            }));
        });
        terminal::enable_raw_mode()
            .map_err(|e| StructuredDiagnostic::from_io("E_RAW_MODE", &e, None))?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

pub enum ReadLineResult {
    Line(String),
    Eof,
}

pub struct LineEditor {
    pub history: History,
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl LineEditor {
    pub fn new() -> Self {
        Self {
            history: History::load_default(),
        }
    }

    pub fn read_line(
        &mut self,
        prompt: &str,
        cwd: &Path,
    ) -> Result<ReadLineResult, StructuredDiagnostic> {
        let _guard = RawModeGuard::enter()?;
        let mut stdout = io::stdout();
        let mut buf = String::new();
        let mut cur = 0usize;
        let mut hist_idx: Option<usize> = None;
        let mut saved_draft = String::new();
        let mut search: Option<Search> = None;

        redraw(&mut stdout, prompt, &buf, cur)?;

        loop {
            if !event::poll(Duration::from_millis(16))
                .map_err(|e| StructuredDiagnostic::from_io("E_TERM_POLL", &e, None))?
            {
                continue;
            }
            let ev = event::read()
                .map_err(|e| StructuredDiagnostic::from_io("E_TERM_EVENT", &e, None))?;
            let Event::Key(KeyEvent {
                code, modifiers, ..
            }) = ev
            else {
                continue;
            };

            if let Some(mut s) = search.take() {
                match s.key(&self.history, code, modifiers) {
                    SearchKey::Stay => {
                        s.draw(&mut stdout, &self.history)?;
                        search = Some(s);
                        continue;
                    }
                    SearchKey::Cancel => {
                        buf = s.origin;
                        cur = s.origin_cur;
                        redraw(&mut stdout, prompt, &buf, cur)?;
                        continue;
                    }
                    SearchKey::Run => {
                        let line = s.shown(&self.history).0.to_string();
                        redraw(&mut stdout, prompt, &line, line.len())?;
                        let _ = write!(stdout, "\r\n");
                        let _ = stdout.flush();
                        return Ok(ReadLineResult::Line(line));
                    }
                    SearchKey::Abort => {
                        buf = s.origin;
                        cur = s.origin_cur;
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    }
                    SearchKey::Leave => {
                        match s.found.and_then(|i| self.history.get(i).map(|e| (i, e))) {
                            Some((i, e)) => {
                                if hist_idx.is_none() {
                                    saved_draft = s.origin;
                                }
                                buf = e.command.clone();
                                cur = s.at;
                                hist_idx = Some(i);
                            }
                            None => {
                                buf = s.origin;
                                cur = s.origin_cur;
                            }
                        }
                        redraw(&mut stdout, prompt, &buf, cur)?;
                        if code == KeyCode::Char('j') && modifiers.contains(KeyModifiers::CONTROL) {
                            continue;
                        }
                    }
                }
            }

            match (code, modifiers) {
                (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                    buf.clear();
                    cur = 0;
                    hist_idx = None;
                    let _ = write!(stdout, "^C\r\n");
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        let _ = write!(stdout, "\r\n");
                        let _ = stdout.flush();
                        return Ok(ReadLineResult::Eof);
                    }
                }
                (KeyCode::Enter, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                    let _ = write!(stdout, "\r\n");
                    let _ = stdout.flush();
                    return Ok(ReadLineResult::Line(buf));
                }
                (KeyCode::Backspace, _) => {
                    if cur > 0 {
                        let prev = prev_char_boundary(&buf, cur);
                        buf.drain(prev..cur);
                        cur = prev;
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    }
                }
                (KeyCode::Delete, _) => {
                    if cur < buf.len() {
                        let next = next_char_boundary(&buf, cur);
                        buf.drain(cur..next);
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    }
                }
                (KeyCode::Left, _) => {
                    if cur > 0 {
                        cur = prev_char_boundary(&buf, cur);
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    }
                }
                (KeyCode::Right, _) => {
                    if cur < buf.len() {
                        cur = next_char_boundary(&buf, cur);
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    }
                }
                (KeyCode::Home, _) => {
                    cur = 0;
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::End, _) => {
                    cur = buf.len();
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Up, _) => {
                    if !self.history.is_empty() {
                        let next_idx = match hist_idx {
                            None => {
                                saved_draft = buf.clone();
                                self.history.len().saturating_sub(1)
                            }
                            Some(i) => i.saturating_sub(1),
                        };
                        hist_idx = Some(next_idx);
                        if let Some(entry) = self.history.get(next_idx) {
                            buf = entry.command.clone();
                            cur = buf.len();
                            redraw(&mut stdout, prompt, &buf, cur)?;
                        }
                    }
                }
                (KeyCode::Down, _) => {
                    if let Some(i) = hist_idx {
                        if i + 1 < self.history.len() {
                            let next_idx = i + 1;
                            hist_idx = Some(next_idx);
                            if let Some(entry) = self.history.get(next_idx) {
                                buf = entry.command.clone();
                                cur = buf.len();
                            }
                        } else {
                            hist_idx = None;
                            buf = saved_draft.clone();
                            cur = buf.len();
                        }
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    }
                }
                (KeyCode::Tab, _) => {
                    let candidates = complete_line(&buf, cur, cwd);
                    if candidates.len() == 1 {
                        apply_completion(&mut buf, &mut cur, &candidates[0], false);
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    } else if candidates.len() > 1 {
                        let lcp = longest_common_prefix(&candidates);
                        let token_start = current_token_start(&buf, cur);
                        let current_tok = &buf[token_start..cur];
                        if lcp.len() > current_tok.len() {
                            apply_completion(&mut buf, &mut cur, &lcp, true);
                        } else {
                            let _ = write!(stdout, "\r\n{}\r\n", candidates.join("  "));
                        }
                        redraw(&mut stdout, prompt, &buf, cur)?;
                    }
                }
                (KeyCode::Char('r'), m) if m.contains(KeyModifiers::CONTROL) => {
                    let s = Search::new(&buf, cur);
                    s.draw(&mut stdout, &self.history)?;
                    search = Some(s);
                }
                (KeyCode::Char('a'), m) if m.contains(KeyModifiers::CONTROL) => {
                    cur = 0;
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char('e'), m) if m.contains(KeyModifiers::CONTROL) => {
                    cur = buf.len();
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                    buf.drain(..cur);
                    cur = 0;
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char('k'), m) if m.contains(KeyModifiers::CONTROL) => {
                    buf.truncate(cur);
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char('w'), m) if m.contains(KeyModifiers::CONTROL) => {
                    let start = word_back(&buf, cur, char::is_whitespace);
                    buf.drain(start..cur);
                    cur = start;
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char('b'), m) if m.contains(KeyModifiers::ALT) => {
                    cur = word_back(&buf, cur, not_alnum);
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char('f'), m) if m.contains(KeyModifiers::ALT) => {
                    cur = word_forward(&buf, cur, not_alnum);
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                    buf.insert(cur, c);
                    cur += c.len_utf8();
                    redraw(&mut stdout, prompt, &buf, cur)?;
                }
                _ => {}
            }
        }
    }
}

fn redraw(
    stdout: &mut io::Stdout,
    prompt: &str,
    buf: &str,
    cur: usize,
) -> Result<(), StructuredDiagnostic> {
    let highlighted = highlight_line(buf);
    let char_offset = buf.get(..cur).map(|s| s.chars().count()).unwrap_or(0);
    let prompt_chars = prompt.chars().count();
    let target_col = (prompt_chars + char_offset) as u16;

    queue!(
        stdout,
        cursor::MoveToColumn(0),
        Clear(ClearType::UntilNewLine),
    )
    .map_err(|e| StructuredDiagnostic::from_io("E_TERM_WRITE", &e, None))?;
    write!(stdout, "{prompt}{highlighted}")
        .map_err(|e| StructuredDiagnostic::from_io("E_TERM_WRITE", &e, None))?;
    queue!(stdout, cursor::MoveToColumn(target_col))
        .map_err(|e| StructuredDiagnostic::from_io("E_TERM_WRITE", &e, None))?;
    stdout
        .flush()
        .map_err(|e| StructuredDiagnostic::from_io("E_TERM_WRITE", &e, None))?;
    Ok(())
}

enum SearchKey {
    Stay,
    Cancel,
    Abort,
    Run,
    Leave,
}

struct Search {
    query: String,
    found: Option<usize>,
    at: usize,
    failed: bool,
    origin: String,
    origin_cur: usize,
}

impl Search {
    fn new(origin: &str, origin_cur: usize) -> Self {
        Self {
            query: String::new(),
            found: None,
            at: 0,
            failed: false,
            origin: origin.to_string(),
            origin_cur,
        }
    }

    fn key(&mut self, history: &History, code: KeyCode, mods: KeyModifiers) -> SearchKey {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Esc => SearchKey::Cancel,
            KeyCode::Char('g') if ctrl => SearchKey::Cancel,
            KeyCode::Char('c') if ctrl => SearchKey::Abort,
            KeyCode::Enter => SearchKey::Run,
            KeyCode::Char('r') if ctrl => {
                self.step(history, self.found, true);
                SearchKey::Stay
            }
            KeyCode::Backspace => {
                if self.query.pop().is_some() {
                    if self.query.is_empty() {
                        self.failed = false;
                    } else {
                        self.found = None;
                        self.step(history, None, false);
                    }
                }
                SearchKey::Stay
            }
            KeyCode::Char(c) if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.query.push(c);
                self.step(history, self.found, false);
                SearchKey::Stay
            }
            _ => SearchKey::Leave,
        }
    }

    fn step(&mut self, history: &History, from: Option<usize>, older: bool) {
        if self.query.is_empty() {
            return;
        }
        let (end, skip) = match from {
            Some(i) if older => (i, history.get(i).map(|e| e.command.as_str())),
            Some(i) => (i + 1, None),
            None => (history.len(), None),
        };
        match history.search_back(&self.query, end, skip) {
            Some(i) => {
                self.found = Some(i);
                self.at = history
                    .get(i)
                    .and_then(|e| e.command.find(&self.query))
                    .unwrap_or(0);
                self.failed = false;
            }
            None => self.failed = true,
        }
    }

    fn shown<'a>(&'a self, history: &'a History) -> (&'a str, usize) {
        match self.found.and_then(|i| history.get(i)) {
            Some(e) => (e.command.as_str(), self.at),
            None => (self.origin.as_str(), self.origin_cur),
        }
    }

    fn draw(&self, stdout: &mut io::Stdout, history: &History) -> Result<(), StructuredDiagnostic> {
        let (text, at) = self.shown(history);
        let label = if self.failed {
            "failed reverse-i-search"
        } else {
            "reverse-i-search"
        };
        let prompt = format!("({label})'{}': ", self.query);
        redraw(stdout, &prompt, text, at)
    }
}

fn word_back(buf: &str, cur: usize, is_sep: fn(char) -> bool) -> usize {
    let mut pos = cur;
    let mut chars = buf[..cur].char_indices().rev().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if !is_sep(c) {
            break;
        }
        pos = i;
        chars.next();
    }
    while let Some(&(i, c)) = chars.peek() {
        if is_sep(c) {
            break;
        }
        pos = i;
        chars.next();
    }
    pos
}

fn word_forward(buf: &str, cur: usize, is_sep: fn(char) -> bool) -> usize {
    let mut pos = cur;
    let mut chars = buf[cur..].char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if !is_sep(c) {
            break;
        }
        pos = cur + i + c.len_utf8();
        chars.next();
    }
    while let Some(&(i, c)) = chars.peek() {
        if is_sep(c) {
            break;
        }
        pos = cur + i + c.len_utf8();
        chars.next();
    }
    pos
}

fn not_alnum(c: char) -> bool {
    !c.is_alphanumeric()
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = (idx + 1).min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn current_token_start(buf: &str, cur: usize) -> usize {
    let prefix = buf.get(..cur).unwrap_or(buf);
    prefix
        .rfind(|c: char| c.is_whitespace() || c == '|')
        .map(|i| i + 1)
        .unwrap_or(0)
}

fn apply_completion(buf: &mut String, cur: &mut usize, completion: &str, partial: bool) {
    let start = current_token_start(buf, *cur);
    let mut replacement = completion.to_string();
    if !partial && !replacement.ends_with('/') {
        replacement.push(' ');
    }
    buf.replace_range(start..*cur, &replacement);
    *cur = start + replacement.len();
}

fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut len = first.len();
    for item in items.iter().skip(1) {
        len = len.min(item.len());
        for (i, (a, b)) in first.bytes().zip(item.bytes()).enumerate() {
            if a != b {
                len = len.min(i);
                break;
            }
        }
    }
    first.get(..len).unwrap_or("").to_string()
}
