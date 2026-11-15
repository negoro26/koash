use std::io::{BufWriter, Write};

use crate::RecordBatch;

pub fn render_batch_table(batch: &RecordBatch, writer: &mut dyn Write, color: bool) {
    let ncols = batch.ncols();
    let nrows = batch.nrows();
    if ncols == 0 || nrows == 0 {
        return;
    }

    let mut widths: Vec<usize> = batch.columns.iter().map(|c| c.len()).collect();
    let mut scratch = String::new();
    for row in batch.rows() {
        for (c, cell) in row.iter().enumerate() {
            let len = cell.text_len(&mut scratch);
            if len > widths[c] {
                widths[c] = len;
            }
        }
    }

    let mut bw = BufWriter::with_capacity(8192, writer);
    for (i, col) in batch.columns.iter().enumerate() {
        if i > 0 {
            let _ = bw.write_all(b"  ");
        }
        if color {
            let _ = bw.write_all(b"\x1b[1;36m");
            let _ = bw.write_all(col.as_bytes());
            let _ = bw.write_all(b"\x1b[0m");
        } else {
            let _ = bw.write_all(col.as_bytes());
        }
        let pad = widths[i].saturating_sub(col.len());
        write_spaces(&mut bw, pad);
    }
    let _ = bw.write_all(b"\n");

    for (i, &w) in widths.iter().enumerate() {
        if i > 0 {
            let _ = bw.write_all(b"  ");
        }
        write_dashes(&mut bw, w);
    }
    let _ = bw.write_all(b"\n");

    for row in batch.rows() {
        for (c, cell) in row.iter().enumerate() {
            if c > 0 {
                let _ = bw.write_all(b"  ");
            }
            let written = cell.write_text(&mut bw, &mut scratch);
            let pad = widths[c].saturating_sub(written);
            write_spaces(&mut bw, pad);
        }
        let _ = bw.write_all(b"\n");
    }
    let _ = bw.flush();
}

fn write_spaces<W: Write>(w: &mut W, mut count: usize) {
    const SPACES: &[u8; 64] = b"                                                                ";
    while count > 0 {
        let n = count.min(SPACES.len());
        let _ = w.write_all(&SPACES[..n]);
        count -= n;
    }
}

fn write_dashes<W: Write>(w: &mut W, mut count: usize) {
    const DASHES: &[u8; 64] = b"----------------------------------------------------------------";
    while count > 0 {
        let n = count.min(DASHES.len());
        let _ = w.write_all(&DASHES[..n]);
        count -= n;
    }
}

pub fn batch_to_compact_sigil(batch: &RecordBatch) -> String {
    if batch.is_empty() {
        return String::new();
    }
    let mut out = String::from("@schema:");
    for (i, col) in batch.columns.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        out.push_str(col.as_ref());
    }
    out.push('\n');
    let mut scratch = String::new();
    for row in batch.rows() {
        for (c, cell) in row.iter().enumerate() {
            if c > 0 {
                out.push('¦');
            }
            if !cell.is_null() {
                let text = cell.text(&mut scratch);
                if text.contains('\n') {
                    out.push_str(&text.replace('\n', "\\n"));
                } else {
                    out.push_str(text);
                }
            }
        }
        out.push('\n');
    }
    out
}
