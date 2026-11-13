use std::borrow::Cow;
use std::sync::Arc;

use serde_json::Value as JsonVal;
use shell_value::{CellVal, RecordBatch, StructuredDiagnostic, Value, batch_to_compact_sigil};

#[derive(Clone, Copy)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
}

impl Cmp {
    fn parse(op: &str) -> Option<Self> {
        Some(match op {
            "==" | "=" => Self::Eq,
            "!=" => Self::Ne,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            "=~" | "contains" => Self::Contains,
            _ => return None,
        })
    }

    fn holds<T: PartialOrd>(self, lhs: T, rhs: T) -> bool {
        match self {
            Self::Eq => lhs == rhs,
            Self::Ne => lhs != rhs,
            Self::Lt => lhs < rhs,
            Self::Le => lhs <= rhs,
            Self::Gt => lhs > rhs,
            Self::Ge => lhs >= rhs,
            Self::Contains => false,
        }
    }
}

pub(crate) fn where_filter(args: &[String], input: Value) -> Result<Value, StructuredDiagnostic> {
    let [field, op, rhs_raw] = args else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "where requires `<field> <op> <value>`",
            None,
            Some("example: ls | where size > 10kb".to_string()),
        ));
    };
    let Some(cmp) = Cmp::parse(op) else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            format!("where: unknown operator `{op}`"),
            None,
            Some("operators: == != < <= > >= =~".to_string()),
        ));
    };
    let batch = input.into_batch();
    let Some(col_idx) = batch.col_index(field) else {
        return Ok(Value::from_batch(RecordBatch::new(batch.columns)));
    };

    let mut out_cells = Vec::new();
    for r in 0..batch.nrows() {
        let row = batch.row(r);
        if eval_cell_predicate(&row[col_idx], cmp, rhs_raw) {
            out_cells.extend_from_slice(row);
        }
    }
    Ok(Value::from_batch(RecordBatch {
        columns: batch.columns,
        cells: out_cells,
    }))
}

fn eval_cell_predicate(lhs: &CellVal, cmp: Cmp, rhs_raw: &str) -> bool {
    if !matches!(cmp, Cmp::Contains)
        && let Some(lhs_num) = lhs.as_f64()
        && let Some(rhs_num) = parse_size_or_number(rhs_raw)
    {
        return cmp.holds(lhs_num, rhs_num);
    }

    let owned;
    let lhs_str = if let Some(s) = lhs.as_str() {
        s
    } else {
        owned = lhs.format_to_string();
        &owned
    };
    match cmp {
        Cmp::Contains => lhs_str.contains(rhs_raw),
        _ => cmp.holds(lhs_str, rhs_raw),
    }
}

fn parse_size_or_number(s: &str) -> Option<f64> {
    let lower = s.trim().to_ascii_lowercase();
    if let Ok(n) = lower.parse::<f64>() {
        return Some(n);
    }
    let suffixes = [
        ("gb", 1024.0 * 1024.0 * 1024.0),
        ("mb", 1024.0 * 1024.0),
        ("kb", 1024.0),
        ("b", 1.0),
    ];
    for (suf, mult) in suffixes {
        if let Some(num_part) = lower.strip_suffix(suf)
            && let Ok(n) = num_part.trim().parse::<f64>()
        {
            return Some(n * mult);
        }
    }
    None
}

pub(crate) fn select(args: &[String], input: Value) -> Result<Value, StructuredDiagnostic> {
    if args.is_empty() {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "select requires at least one column name",
            None,
            Some("example: ls | select name size".to_string()),
        ));
    }
    let batch = input.into_batch();
    let mut keep_cols = Vec::new();
    let mut keep_indices = Vec::new();
    for col in args {
        if let Some(idx) = batch.col_index(col) {
            keep_cols.push(Cow::Owned(col.clone()));
            keep_indices.push(idx);
        }
    }
    if keep_cols.is_empty() {
        return Ok(Value::from_batch(RecordBatch::empty()));
    }
    let nrows = batch.nrows();
    let mut out_cells = Vec::with_capacity(nrows * keep_cols.len());
    for r in 0..nrows {
        let row = batch.row(r);
        for &idx in &keep_indices {
            out_cells.push(row[idx].clone());
        }
    }
    Ok(Value::from_batch(RecordBatch {
        columns: Arc::from(keep_cols),
        cells: out_cells,
    }))
}

pub(crate) fn sort_by(args: &[String], input: Value) -> Result<Value, StructuredDiagnostic> {
    let mut desc = false;
    let mut col: Option<String> = None;
    for arg in args {
        if arg == "--desc" || arg == "-r" {
            desc = true;
        } else {
            col = Some(arg.clone());
        }
    }
    let Some(col) = col else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "sort-by requires a column name",
            None,
            None,
        ));
    };
    let batch = input.into_batch();
    let Some(col_idx) = batch.col_index(&col) else {
        return Ok(Value::from_batch(batch));
    };
    let nrows = batch.nrows();
    let ncols = batch.ncols();
    let mut order: Vec<usize> = (0..nrows).collect();
    order.sort_by(|&a, &b| {
        let va = &batch.row(a)[col_idx];
        let vb = &batch.row(b)[col_idx];
        let ord = match (va.as_f64(), vb.as_f64()) {
            (Some(na), Some(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
            _ => {
                let sa = va.as_str().unwrap_or("");
                let sb = vb.as_str().unwrap_or("");
                sa.cmp(sb)
            }
        };
        if desc { ord.reverse() } else { ord }
    });
    let mut out_cells = Vec::with_capacity(nrows * ncols);
    for idx in order {
        out_cells.extend_from_slice(batch.row(idx));
    }
    Ok(Value::from_batch(RecordBatch {
        columns: batch.columns,
        cells: out_cells,
    }))
}

pub(crate) fn first(args: &[String], input: Value) -> Result<Value, StructuredDiagnostic> {
    let n = args
        .first()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);
    let mut batch = input.into_batch();
    batch.truncate_rows(n);
    Ok(Value::from_batch(batch))
}

pub(crate) fn last(args: &[String], input: Value) -> Result<Value, StructuredDiagnostic> {
    let n = args
        .first()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);
    let mut batch = input.into_batch();
    batch.tail_rows(n);
    Ok(Value::from_batch(batch))
}

pub(crate) fn from(args: &[String], input: Value) -> Result<Value, StructuredDiagnostic> {
    let fmt = args.first().map(|s| s.as_str()).unwrap_or("json");
    if fmt != "json" {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            format!("unsupported `from` format `{fmt}`"),
            None,
            None,
        ));
    }
    let bytes = input.into_raw_bytes();
    let text = String::from_utf8_lossy(&bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Value::from_batch(RecordBatch::empty()));
    }
    if let Ok(val) = serde_json::from_str::<JsonVal>(trimmed) {
        match val {
            JsonVal::Array(arr) => {
                let mut objs = Vec::with_capacity(arr.len());
                for v in arr {
                    if let JsonVal::Object(m) = v {
                        objs.push(m);
                    }
                }
                return Ok(Value::from_batch(batch_from_json_objects(objs)));
            }
            JsonVal::Object(map) => {
                return Ok(Value::from_batch(batch_from_json_objects(vec![map])));
            }
            _ => {}
        }
    }
    let mut objs = Vec::new();
    for line in trimmed.lines() {
        if let Ok(JsonVal::Object(map)) = serde_json::from_str::<JsonVal>(line) {
            objs.push(map);
        }
    }
    Ok(Value::from_batch(batch_from_json_objects(objs)))
}

fn batch_from_json_objects(objs: Vec<serde_json::Map<String, JsonVal>>) -> RecordBatch {
    if objs.is_empty() {
        return RecordBatch::empty();
    }
    let mut columns: Vec<Cow<'static, str>> = Vec::new();
    for map in &objs {
        for k in map.keys() {
            if !columns.iter().any(|c| c.as_ref() == k.as_str()) {
                columns.push(Cow::Owned(k.clone()));
            }
        }
    }
    let ncols = columns.len();
    let mut cells = Vec::with_capacity(objs.len() * ncols);
    for mut map in objs {
        for col in &columns {
            let cell = match map.remove(col.as_ref()) {
                Some(v) => CellVal::from_json(v),
                None => CellVal::Null,
            };
            cells.push(cell);
        }
    }
    RecordBatch {
        columns: Arc::from(columns),
        cells,
    }
}

pub(crate) fn to(args: &[String], input: Value) -> Result<Value, StructuredDiagnostic> {
    let fmt = args.first().map(|s| s.as_str()).unwrap_or("json");
    let batch = input.into_batch();
    match fmt {
        "json" => {
            let records = batch.to_maps();
            let mut json = serde_json::to_string_pretty(&records)
                .map_err(|e| StructuredDiagnostic::new("E_JSON", e.to_string(), None, None))?;
            json.push('\n');
            Ok(Value::from_bytes(json.into_bytes()))
        }
        "compact" => {
            let sigil = batch_to_compact_sigil(&batch);
            Ok(Value::from_bytes(sigil.into_bytes()))
        }
        other => Err(StructuredDiagnostic::new(
            "E_ARGS",
            format!("unsupported `to` format `{other}`"),
            None,
            None,
        )),
    }
}
