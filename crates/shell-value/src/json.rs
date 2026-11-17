use std::collections::BTreeMap;
use std::slice;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::{Column, Datum, RecordBatch, Row, RowSink, StructuredDiagnostic};

pub type StructuredRecord = BTreeMap<String, serde_json::Value>;

pub(crate) fn batch_from_raw_bytes(bytes: &[u8]) -> Result<RecordBatch, StructuredDiagnostic> {
    let text = String::from_utf8_lossy(bytes);
    let mut records = Vec::new();
    for line in text.lines() {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(line)
        {
            let rec: StructuredRecord = map.into_iter().collect();
            records.push(rec);
        } else {
            let mut rec = StructuredRecord::new();
            rec.insert(
                "line".to_string(),
                serde_json::Value::String(line.to_string()),
            );
            records.push(rec);
        }
    }
    RecordBatch::from_maps(records)
}

pub(crate) fn put_value(out: &mut RowSink<'_>, value: serde_json::Value) {
    match value {
        serde_json::Value::Null => out.put(Datum::Null),
        serde_json::Value::Bool(b) => out.put(Datum::Bool(b)),
        serde_json::Value::Number(n) => out.put(if let Some(u) = n.as_u64() {
            Datum::UInt(u)
        } else if let Some(i) = n.as_i64() {
            Datum::Int(i)
        } else {
            Datum::Float(n.as_f64().unwrap_or(0.0))
        }),
        serde_json::Value::String(s) => out.put_str(&s),
        nested => out.put(Datum::Json(&nested.to_string())),
    }
}

impl Serialize for Datum<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match *self {
            Datum::Null => s.serialize_unit(),
            Datum::Bool(b) => s.serialize_bool(b),
            Datum::Int(n) => s.serialize_i64(n),
            Datum::UInt(n) => s.serialize_u64(n),
            Datum::Float(n) => s.serialize_f64(n),
            Datum::Str(text) => s.serialize_str(text),
            Datum::Json(text) => match serde_json::from_str::<serde_json::Value>(text) {
                Ok(value) => value.serialize(s),
                Err(_) => s.serialize_unit(),
            },
            Datum::Time(_) | Datum::Mode(_) => {
                s.serialize_str(self.short_text().unwrap_or_default().as_str())
            }
        }
    }
}

pub(crate) fn name_order(batch: &RecordBatch) -> Vec<usize> {
    let columns = batch.schema().columns();
    let mut order: Vec<usize> = (0..columns.len()).collect();
    order.sort_by(|&a, &b| columns[a].name().cmp(columns[b].name()));
    order
}

pub(crate) fn visible_columns<'a>(
    columns: &'a [Column],
    order: &'a [usize],
    row: Row<'a>,
) -> impl Iterator<Item = usize> + 'a {
    let mut rest = order;
    std::iter::from_fn(move || {
        while let Some(&first) = rest.first() {
            let name = &columns[first];
            let same = rest.iter().take_while(|&&c| columns[c] == *name).count();
            let (group, tail) = rest.split_at(same);
            rest = tail;
            if let Some(&col) = group.iter().rev().find(|&&c| !row.get(c).is_null()) {
                return Some(col);
            }
        }
        None
    })
}

struct JsonRow<'a> {
    columns: &'a [Column],
    order: &'a [usize],
    row: Row<'a>,
}

impl Serialize for JsonRow<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        for col in visible_columns(self.columns, self.order, self.row) {
            map.serialize_entry(self.columns[col].name(), &self.row.get(col))?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Records<'a> {
    batches: &'a [RecordBatch],
    limit: usize,
}

impl<'a> Records<'a> {
    pub fn of(batches: &'a [RecordBatch]) -> Self {
        Self {
            batches,
            limit: usize::MAX,
        }
    }

    pub fn take(self, rows: usize) -> Self {
        Self {
            limit: self.limit.min(rows),
            ..self
        }
    }

    pub fn len(&self) -> usize {
        self.batches
            .iter()
            .map(RecordBatch::nrows)
            .sum::<usize>()
            .min(self.limit)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn json_lines(&self) -> impl Iterator<Item = String> + 'a {
        self.batches
            .iter()
            .flat_map(|batch| {
                let order = name_order(batch);
                batch.rows().map(move |row| {
                    let json = JsonRow {
                        columns: batch.schema().columns(),
                        order: &order,
                        row,
                    };
                    serde_json::to_string(&json).unwrap_or_default()
                })
            })
            .take(self.limit)
    }
}

impl Serialize for Records<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.len()))?;
        let mut left = self.limit;
        for batch in self.batches {
            let order = name_order(batch);
            for row in batch.rows().take(left) {
                seq.serialize_element(&JsonRow {
                    columns: batch.schema().columns(),
                    order: &order,
                    row,
                })?;
                left -= 1;
            }
        }
        seq.end()
    }
}

impl RecordBatch {
    pub fn records(&self) -> Records<'_> {
        Records::of(slice::from_ref(self))
    }

    pub fn write_ndjson(&self, out: &mut Vec<u8>) {
        let order = name_order(self);
        for row in self.rows() {
            let json = JsonRow {
                columns: self.schema().columns(),
                order: &order,
                row,
            };
            if serde_json::to_writer(&mut *out, &json).is_ok() {
                out.push(b'\n');
            }
        }
    }
}
