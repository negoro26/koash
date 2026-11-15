use std::collections::HashMap;

use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::catalog;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Discover;

static SIGNATURE: Signature = Signature {
    name: "discover",
    aliases: &[],
    description: "BM25 progressive command discovery by natural language intent",
    flags: &["--limit"],
    columns: &["command", "score", "description", "flags", "columns"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Discover {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut limit: usize = 3;
        let mut query_parts = Vec::new();
        let mut iter = args.iter();

        while let Some(arg) = iter.next() {
            if arg == "--limit"
                && let Some(val) = iter.next()
                && let Ok(n) = val.parse::<usize>()
            {
                limit = n.max(1);
            } else if !arg.starts_with('-') {
                query_parts.push(arg.as_str());
            }
        }

        let query = query_parts.join(" ");
        let q_terms = tokenize_terms(&query);
        if q_terms.is_empty() {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "discover requires a search intent",
                None,
                Some("example: discover \"search code hash\"".to_string()),
            ));
        }

        let sigs: Vec<&Signature> = catalog().collect();
        let docs: Vec<Vec<String>> = sigs
            .iter()
            .map(|s| {
                let mut text = format!("{} {}", s.name, s.description);
                for f in s.flags {
                    text.push(' ');
                    text.push_str(f);
                }
                for c in s.columns {
                    text.push(' ');
                    text.push_str(c);
                }
                tokenize_terms(&text)
            })
            .collect();

        let n_docs = docs.len() as f64;
        let avg_dl = docs.iter().map(|d| d.len() as f64).sum::<f64>() / n_docs.max(1.0);
        let mut df: HashMap<&str, usize> = HashMap::new();
        for doc in &docs {
            let mut seen = Vec::new();
            for term in doc {
                if !seen.contains(&term.as_str()) {
                    seen.push(term.as_str());
                    *df.entry(term.as_str()).or_insert(0) += 1;
                }
            }
        }

        let k1 = 1.5;
        let b = 0.75;
        let mut scored: Vec<(f64, usize)> = docs
            .iter()
            .enumerate()
            .map(|(idx, doc)| {
                let dl = doc.len() as f64;
                let mut score = 0.0;
                for qt in &q_terms {
                    let tf = doc.iter().filter(|t| *t == qt).count() as f64;
                    if tf > 0.0 {
                        let doc_freq = *df.get(qt.as_str()).unwrap_or(&0) as f64;
                        let idf = ((n_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();
                        let denom = tf + k1 * (1.0 - b + b * (dl / avg_dl.max(1.0)));
                        score += idf * ((tf * (k1 + 1.0)) / denom);
                    }
                }
                (score, idx)
            })
            .filter(|(s, _)| *s > 0.0)
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let records = scored
            .into_iter()
            .map(|(score, idx)| {
                let s = sigs[idx];
                let mut rec = StructuredRecord::new();
                rec.insert("command".to_string(), JsonVal::String(s.name.to_string()));
                rec.insert("score".to_string(), json!((score * 100.0).round() / 100.0));
                rec.insert(
                    "description".to_string(),
                    JsonVal::String(s.description.to_string()),
                );
                rec.insert("flags".to_string(), JsonVal::String(s.flags.join(" ")));
                rec.insert("columns".to_string(), JsonVal::String(s.columns.join(", ")));
                rec
            })
            .collect();

        Ok(Value::from_records(records))
    }
}

fn tokenize_terms(s: &str) -> Vec<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}
