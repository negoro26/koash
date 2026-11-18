use std::io::{IsTerminal, Write};

use serde::Serialize;
use shell_value::{Datum, RecordBatch, Records, TableStyle, Value, render_fitted};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceMode {
    AnsiTty,
    Plain,
    TernSurfaceProtocol,
}

impl SurfaceMode {
    pub fn detect(force_tsp: bool, color: bool) -> Self {
        if force_tsp || std::env::var("TERM_PROGRAM").as_deref() == Ok("Tern") {
            Self::TernSurfaceProtocol
        } else if color {
            Self::AnsiTty
        } else {
            Self::Plain
        }
    }
}

#[derive(Serialize)]
struct TspTableFrame<'a> {
    tsp: &'static str,
    kind: &'static str,
    columns: &'a [String],
    rows: Records<'a>,
}

#[derive(Serialize)]
struct TspTextFrame<'a> {
    tsp: &'static str,
    kind: &'static str,
    content: &'a str,
}

pub fn terminal_width() -> Option<usize> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&w| w > 0)
        .or_else(|| {
            crossterm::terminal::size()
                .ok()
                .map(|(cols, _)| usize::from(cols))
                .filter(|&w| w > 0)
        })
}

pub fn render_surface(val: Value, writer: &mut dyn Write, mode: SurfaceMode, width: Option<usize>) {
    match mode {
        SurfaceMode::AnsiTty => val.render_tty(writer, true, width),
        SurfaceMode::Plain => val.render_tty(writer, false, width),
        SurfaceMode::TernSurfaceProtocol => render_tsp(val, writer, width),
    }
}

fn render_tsp(val: Value, writer: &mut dyn Write, width: Option<usize>) {
    match val {
        Value::Empty => {}
        Value::Batch(batch) => {
            if batch.is_empty() {
                return;
            }
            let columns: Vec<String> = batch.schema().names().map(str::to_string).collect();
            let frame = TspTableFrame {
                tsp: "1.0",
                kind: "table",
                columns: &columns,
                rows: batch.records(),
            };
            if let Ok(json) = serde_json::to_string(&frame) {
                let _ = write!(writer, "\x1b]9001;{json}\x07");
                let preview = match width {
                    Some(width) => fitted_preview(&batch, width),
                    None => render_unicode_preview(&columns, &batch),
                };
                let _ = writeln!(writer, "{preview}");
                let _ = writer.flush();
            }
        }
        Value::Bytes(buf) => {
            let text = String::from_utf8_lossy(&buf);
            let frame = TspTextFrame {
                tsp: "1.0",
                kind: "text",
                content: &text,
            };
            if let Ok(json) = serde_json::to_string(&frame) {
                let _ = write!(writer, "\x1b]9001;{json}\x07");
            }
            let _ = writer.write_all(&buf);
            let _ = writer.flush();
        }
    }
}

fn fitted_preview(batch: &RecordBatch, width: usize) -> String {
    let mut out = Vec::new();
    render_fitted(batch, &mut out, false, width, &TableStyle::BOX);
    String::from_utf8_lossy(&out)
        .trim_end_matches('\n')
        .to_string()
}

fn preview_text(cell: Datum<'_>) -> String {
    match cell {
        Datum::Null => String::new(),
        Datum::Float(f) if !f.is_finite() => String::new(),
        Datum::Str(s) => s.to_string(),
        other => match other.short_text() {
            Some(text) => text.as_str().to_string(),
            None => serde_json::to_string(&other).unwrap_or_default(),
        },
    }
}

fn render_unicode_preview(columns: &[String], batch: &RecordBatch) -> String {
    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    let rows: Vec<Vec<String>> = batch
        .rows()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, cell)| {
                    let s = preview_text(cell);
                    widths[i] = widths[i].max(s.len());
                    s
                })
                .collect()
        })
        .collect();

    let mut out = String::new();
    for (i, col) in columns.iter().enumerate() {
        if i > 0 {
            out.push_str(" │ ");
        }
        out.push_str(col);
        for _ in 0..widths[i].saturating_sub(col.len()) {
            out.push(' ');
        }
    }
    out.push('\n');
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            out.push_str("─┼─");
        }
        out.push_str(&"─".repeat(*w));
    }
    for row in rows {
        out.push('\n');
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str(" │ ");
            }
            out.push_str(cell);
            for _ in 0..widths[i].saturating_sub(cell.len()) {
                out.push(' ');
            }
        }
    }
    out
}
