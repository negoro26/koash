use std::io::Write;

use serde::Serialize;
use shell_value::{Datum, RecordBatch, Records, Value};

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

pub fn render_surface(val: Value, writer: &mut dyn Write, mode: SurfaceMode) {
    match mode {
        SurfaceMode::AnsiTty => val.render_tty(writer, true),
        SurfaceMode::Plain => val.render_tty(writer, false),
        SurfaceMode::TernSurfaceProtocol => render_tsp(val, writer),
    }
}

fn render_tsp(val: Value, writer: &mut dyn Write) {
    match val {
        Value::Empty => {}
        Value::Batch(batch) => {
            if batch.is_empty() {
                return;
            }
            let columns: Vec<String> = batch.columns.iter().map(|c| c.to_string()).collect();
            let frame = TspTableFrame {
                tsp: "1.0",
                kind: "table",
                columns: &columns,
                rows: batch.records(),
            };
            if let Ok(json) = serde_json::to_string(&frame) {
                let _ = write!(writer, "\x1b]9001;{json}\x07");
                let _ = writeln!(writer, "{}", render_unicode_preview(&columns, &batch));
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

/// The cell as the preview prints it: JSON text for everything but strings, so a float shows as `2.0`.
/// A non-finite float was `null` in the old record map and printed as nothing.
/// A repeated column is read by position; its copies hold the same value, since only `select` repeats one.
fn preview_text(cell: Datum<'_>) -> String {
    match cell {
        Datum::Null => String::new(),
        Datum::Float(f) if !f.is_finite() => String::new(),
        Datum::Str(s) => s.to_string(),
        other => serde_json::to_string(&other).unwrap_or_default(),
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
