use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::queue;
use crossterm::terminal::{self, Clear, ClearType};
use shell_value::StructuredDiagnostic;
use tokio::sync::mpsc;

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
        let (req_tx, mut req_rx) = mpsc::channel::<(u64, String, usize, PathBuf)>(4);
        let (resp_tx, mut resp_rx) = mpsc::channel::<(u64, String, Vec<String>)>(4);
        let mut latest_req_id: u64 = 0;

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let handle_clone = handle.clone();
            handle.spawn(async move {
                while let Some((req_id, line, pos, dir)) = req_rx.recv().await {
                    let line_copy = line.clone();
                    if let Ok(candidates) = handle_clone
                        .spawn_blocking(move || complete_line(&line, pos, &dir))
                        .await
                        && resp_tx.send((req_id, line_copy, candidates)).await.is_err()
                    {
                        break;
                    }
                }
            });
        }

        redraw(&mut stdout, prompt, &buf, cur)?;

        loop {
            while let Ok((req_id, req_buf, candidates)) = resp_rx.try_recv() {
                if req_id == latest_req_id && req_buf == buf {
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
            }

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
                (KeyCode::Enter, _) => {
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
                    latest_req_id = latest_req_id.wrapping_add(1);
                    if req_tx
                        .try_send((latest_req_id, buf.clone(), cur, cwd.to_path_buf()))
                        .is_err()
                    {
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
