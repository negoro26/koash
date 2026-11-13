use std::io::Write;

use serde::Serialize;
use shell_value::{StructuredRecord, Value, format_cell};

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
    rows: &'a [StructuredRecord],
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
            let records = batch.to_maps();
            let frame = TspTableFrame {
                tsp: "1.0",
                kind: "table",
                columns: &columns,
                rows: &records,
            };
            if let Ok(json) = serde_json::to_string(&frame) {
                let _ = write!(writer, "\x1b]9001;{json}\x07");
                let _ = writeln!(writer, "{}", render_unicode_preview(&columns, &records));
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

fn render_unicode_preview(columns: &[String], records: &[StructuredRecord]) -> String {
    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    let rows: Vec<Vec<String>> = records
        .iter()
        .map(|rec| {
            columns
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    let s = rec.get(col).map(format_cell).unwrap_or_default();
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
