use shell_kernel::Session;
use shell_value::{RecordBatch, StructuredDiagnostic, Value, batch_to_compact_sigil};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct ToFormat;

static SIGNATURE: Signature = Signature {
    name: "to",
    aliases: &[],
    description: "Serialize structured records (to json, compact, csv, tsv, md)",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for ToFormat {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let fmt = args.first().map(|s| s.as_str()).unwrap_or("json");
        let batch = input.into_batch()?;
        match fmt {
            "json" => {
                let mut json = serde_json::to_string_pretty(&batch.records())
                    .map_err(|e| StructuredDiagnostic::new("E_JSON", e.to_string(), None, None))?;
                json.push('\n');
                Ok(Value::from_bytes(json.into_bytes()))
            }
            "compact" => {
                let sigil = batch_to_compact_sigil(&batch);
                Ok(Value::from_bytes(sigil.into_bytes()))
            }
            "csv" => Ok(Value::from_bytes(
                format_delimited(&batch, ',').into_bytes(),
            )),
            "tsv" => Ok(Value::from_bytes(
                format_delimited(&batch, '\t').into_bytes(),
            )),
            "md" => Ok(Value::from_bytes(format_markdown(&batch).into_bytes())),
            other => Err(StructuredDiagnostic::new(
                "E_ARGS",
                format!("unsupported `to` format `{other}`"),
                None,
                Some("formats: json, compact, csv, tsv, md".to_string()),
            )),
        }
    }
}

fn format_delimited(batch: &RecordBatch, delim: char) -> String {
    if batch.ncols() == 0 {
        return String::new();
    }
    let quote_empty = batch.ncols() == 1;
    let mut out = String::new();
    for (i, name) in batch.schema().names().enumerate() {
        if i > 0 {
            out.push(delim);
        }
        push_delimited_cell(&mut out, name, delim, quote_empty);
    }
    out.push('\n');
    let mut scratch = String::new();
    for row in batch.rows() {
        for (i, datum) in row.iter().enumerate() {
            if i > 0 {
                out.push(delim);
            }
            push_delimited_cell(&mut out, datum.text(&mut scratch), delim, quote_empty);
        }
        out.push('\n');
    }
    out
}

fn push_delimited_cell(out: &mut String, text: &str, delim: char, quote_empty: bool) {
    let needs_quote = (quote_empty && text.is_empty())
        || text.contains(delim)
        || text.contains('"')
        || text.contains('\n')
        || text.contains('\r');
    if !needs_quote {
        out.push_str(text);
        return;
    }
    out.push('"');
    for ch in text.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
}

fn format_markdown(batch: &RecordBatch) -> String {
    if batch.ncols() == 0 {
        return String::new();
    }
    let mut out = String::new();
    out.push('|');
    for name in batch.schema().names() {
        out.push(' ');
        push_md_cell(&mut out, name);
        out.push_str(" |");
    }
    out.push('\n');
    out.push('|');
    for _ in 0..batch.ncols() {
        out.push_str(" --- |");
    }
    out.push('\n');
    let mut scratch = String::new();
    for row in batch.rows() {
        out.push('|');
        for datum in row.iter() {
            out.push(' ');
            push_md_cell(&mut out, datum.text(&mut scratch));
            out.push_str(" |");
        }
        out.push('\n');
    }
    out
}

fn push_md_cell(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '\n' => out.push(' '),
            '\r' => {}
            _ => out.push(ch),
        }
    }
}
