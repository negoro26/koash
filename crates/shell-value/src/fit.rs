use std::borrow::Cow;
use std::io::{BufWriter, Write};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{Datum, RecordBatch};

pub type CellText = for<'s> fn(Datum<'s>, &'s mut String) -> &'s str;

const ELLIPSIS: &str = "…";
const COLOR_HEADER: &[u8] = b"\x1b[1;36m";
const COLOR_RESET: &[u8] = b"\x1b[0m";

#[derive(Debug, Clone, Copy)]
pub struct TableStyle {
    gap: &'static str,
    rule: &'static str,
    rule_gap: &'static str,
}

impl TableStyle {
    pub const PLAIN: TableStyle = TableStyle {
        gap: "  ",
        rule: "-",
        rule_gap: "  ",
    };
    pub const BOX: TableStyle = TableStyle {
        gap: " │ ",
        rule: "─",
        rule_gap: "─┼─",
    };
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn clean(text: &str) -> Cow<'_, str> {
    if text.chars().any(char::is_control) {
        Cow::Owned(
            text.chars()
                .map(|c| if c.is_control() { '?' } else { c })
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

fn prefix_end(text: &str, budget: usize) -> usize {
    let mut used = 0;
    let mut end = 0;
    for (at, ch) in text.char_indices() {
        let next = used + ch.width().unwrap_or(0);
        if next > budget {
            break;
        }
        used = next;
        end = at + ch.len_utf8();
    }
    while end > 0 && text[..end].width() > budget {
        end -= text[..end].chars().next_back().map_or(1, char::len_utf8);
    }
    end
}

fn write_spaces<W: Write>(w: &mut W, count: usize) {
    const SPACES: &str = "                                                                ";
    let mut left = count;
    while left > 0 {
        let n = left.min(SPACES.len());
        let _ = w.write_all(&SPACES.as_bytes()[..n]);
        left -= n;
    }
}

fn write_rule<W: Write>(w: &mut W, rule: &str, count: usize) {
    for _ in 0..count {
        let _ = w.write_all(rule.as_bytes());
    }
}

fn write_clipped<W: Write>(w: &mut W, text: &str, width: usize) -> usize {
    let text = clean(text);
    let natural = text.width();
    if natural <= width {
        let _ = w.write_all(text.as_bytes());
        return natural;
    }
    let end = prefix_end(&text, width.saturating_sub(1));
    let _ = w.write_all(&text.as_bytes()[..end]);
    let _ = w.write_all(ELLIPSIS.as_bytes());
    text[..end].width() + 1
}

fn write_cell<W: Write>(w: &mut W, text: &str, width: usize, pad: bool) {
    let written = write_clipped(w, text, width);
    if pad {
        write_spaces(w, width.saturating_sub(written));
    }
}

fn write_header<W: Write>(w: &mut W, name: &str, width: usize, color: bool, pad: bool) {
    if color {
        let _ = w.write_all(COLOR_HEADER);
    }
    let written = write_clipped(w, name, width);
    if color {
        let _ = w.write_all(COLOR_RESET);
    }
    if pad {
        write_spaces(w, width.saturating_sub(written));
    }
}

fn shrink(natural: &[usize], avail: usize) -> Vec<usize> {
    let sum: usize = natural.iter().sum();
    if sum <= avail {
        return natural.to_vec();
    }
    let mut cap = natural.iter().copied().max().unwrap_or(1);
    while cap > 1 && natural.iter().map(|&w| w.min(cap)).sum::<usize>() > avail {
        cap -= 1;
    }
    let mut slack = avail.saturating_sub(natural.iter().map(|&w| w.min(cap)).sum());
    natural
        .iter()
        .map(|&w| {
            if w > cap && slack > 0 {
                slack -= 1;
                cap + 1
            } else {
                w.min(cap)
            }
        })
        .collect()
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let text = clean(text);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let mut word = word;
        loop {
            let sep = usize::from(!line.is_empty());
            let w = word.width();
            if used + sep + w <= width {
                if sep == 1 {
                    line.push(' ');
                }
                line.push_str(word);
                used += sep + w;
                break;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                used = 0;
                continue;
            }
            let end = match prefix_end(word, width) {
                0 => word.chars().next().map_or(0, char::len_utf8),
                end => end,
            };
            lines.push(word[..end].to_string());
            word = &word[end..];
            if word.is_empty() {
                break;
            }
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

pub fn render_fitted(
    batch: &RecordBatch,
    writer: &mut dyn Write,
    color: bool,
    width: usize,
    style: &TableStyle,
    text: CellText,
) {
    let ncols = batch.ncols();
    if ncols == 0 || batch.nrows() == 0 {
        return;
    }
    let width = width.max(1);
    let gap_width = style.gap.width();
    let columns = batch.schema().columns();

    let mut natural: Vec<usize> = columns.iter().map(|c| clean(c.name()).width()).collect();
    let mut scratch = String::new();
    for row in batch.rows() {
        for (c, cell) in row.iter().enumerate() {
            natural[c] = natural[c].max(clean(text(cell, &mut scratch)).width());
        }
    }

    let mut bw = BufWriter::with_capacity(8192, writer);
    let total = natural.iter().sum::<usize>() + gap_width * (ncols - 1);
    if total <= width {
        write_grid(
            &mut bw,
            batch,
            &(0..ncols).collect::<Vec<_>>(),
            &natural,
            color,
            style,
            text,
        );
    } else if batch.nrows() == 1 && ncols > 1 && width >= gap_width + 3 {
        write_record(&mut bw, batch, color, width, style, text);
    } else {
        let mut shown: Vec<usize> = (0..ncols).collect();
        if shown.iter().any(|&c| columns[c].is_detail())
            && shown.iter().any(|&c| !columns[c].is_detail())
        {
            shown.retain(|&c| !columns[c].is_detail());
        }
        shown.truncate(((width + gap_width) / (1 + gap_width)).max(1));
        let avail = width.saturating_sub(gap_width * (shown.len() - 1));
        let widths = shrink(
            &shown.iter().map(|&c| natural[c]).collect::<Vec<_>>(),
            avail,
        );
        let mut full = vec![0; ncols];
        for (&c, &w) in shown.iter().zip(&widths) {
            full[c] = w;
        }
        write_grid(&mut bw, batch, &shown, &full, color, style, text);
    }
    let _ = bw.flush();
}

fn write_grid<W: Write>(
    w: &mut W,
    batch: &RecordBatch,
    shown: &[usize],
    widths: &[usize],
    color: bool,
    style: &TableStyle,
    text: CellText,
) {
    let columns = batch.schema().columns();
    let last = shown.last().copied();
    for &c in shown {
        if c != shown[0] {
            let _ = w.write_all(style.gap.as_bytes());
        }
        write_header(w, columns[c].name(), widths[c], color, Some(c) != last);
    }
    let _ = w.write_all(b"\n");
    for &c in shown {
        if c != shown[0] {
            let _ = w.write_all(style.rule_gap.as_bytes());
        }
        write_rule(w, style.rule, widths[c]);
    }
    let _ = w.write_all(b"\n");
    let mut scratch = String::new();
    for row in batch.rows() {
        for &c in shown {
            if c != shown[0] {
                let _ = w.write_all(style.gap.as_bytes());
            }
            write_cell(
                w,
                text(row.get(c), &mut scratch),
                widths[c],
                Some(c) != last,
            );
        }
        let _ = w.write_all(b"\n");
    }
}

fn write_record<W: Write>(
    w: &mut W,
    batch: &RecordBatch,
    color: bool,
    width: usize,
    style: &TableStyle,
    text: CellText,
) {
    let columns = batch.schema().columns();
    let gap_width = style.gap.width();
    let key_width = columns
        .iter()
        .map(|c| clean(c.name()).width())
        .max()
        .unwrap_or(1)
        .min((width / 3).max(1))
        .min(width.saturating_sub(gap_width + 2));
    let value_width = width.saturating_sub(key_width + gap_width);
    let mut scratch = String::new();
    for row in batch.rows() {
        for (c, column) in columns.iter().enumerate() {
            for (n, line) in wrap(text(row.get(c), &mut scratch), value_width)
                .iter()
                .enumerate()
            {
                if n == 0 {
                    write_header(w, column.name(), key_width, color, true);
                } else {
                    write_spaces(w, key_width);
                }
                let _ = w.write_all(style.gap.as_bytes());
                let _ = w.write_all(line.as_bytes());
                let _ = w.write_all(b"\n");
            }
        }
    }
}

pub fn plain_cell<'s>(cell: Datum<'s>, scratch: &'s mut String) -> &'s str {
    cell.text(scratch)
}
