use std::borrow::Cow;
use std::collections::BTreeMap;
use std::slice;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::{Datum, RecordBatch, Row};

pub type StructuredRecord = BTreeMap<String, serde_json::Value>;

pub(crate) fn batch_from_raw_bytes(bytes: &[u8]) -> RecordBatch {
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

impl Serialize for Datum<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match *self {
            Datum::Null => s.serialize_unit(),
            Datum::Bool(b) => s.serialize_bool(b),
            Datum::Int(n) => s.serialize_i64(n),
            Datum::UInt(n) => s.serialize_u64(n),
            Datum::Float(n) => s.serialize_f64(n),
            Datum::Str(text) => s.serialize_str(text),
            Datum::Json(v) => v.serialize(s),
        }
    }
}

/// Column indices ordered by name, equal names in column order.
pub(crate) fn name_order(batch: &RecordBatch) -> Vec<usize> {
    let mut order: Vec<usize> = (0..batch.columns.len()).collect();
    order.sort_by(|&a, &b| batch.columns[a].cmp(&batch.columns[b]));
    order
}

/// The column that supplies each JSON key of a row, in key order: one per distinct name, the last
/// non-null cell among columns of that name, and none for a name whose cells are all null.
pub(crate) fn visible_columns<'a>(
    columns: &'a [Cow<'static, str>],
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
    columns: &'a [Cow<'static, str>],
    order: &'a [usize],
    row: Row<'a>,
}

impl Serialize for JsonRow<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        for col in visible_columns(self.columns, self.order, self.row) {
            map.serialize_entry(self.columns[col].as_ref(), &self.row.get(col))?;
        }
        map.end()
    }
}

/// The rows of one or more batches as JSON objects with sorted keys and no null cells.
/// Serializes as an array.
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

    /// Only the first `rows` records.
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

    /// Each record as compact JSON text.
    pub fn json_lines(&self) -> impl Iterator<Item = String> + 'a {
        self.batches
            .iter()
            .flat_map(|batch| {
                let order = name_order(batch);
                batch.rows().map(move |row| {
                    let json = JsonRow {
                        columns: &batch.columns,
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
                    columns: &batch.columns,
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

    /// One compact JSON object per line.
    pub fn write_ndjson(&self, out: &mut Vec<u8>) {
        let order = name_order(self);
        for row in self.rows() {
            let json = JsonRow {
                columns: &self.columns,
                order: &order,
                row,
            };
            if serde_json::to_writer(&mut *out, &json).is_ok() {
                out.push(b'\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn batch(rows: &[Value]) -> RecordBatch {
        let maps = rows
            .iter()
            .map(|row| serde_json::from_value(row.clone()).unwrap_or_default())
            .collect();
        RecordBatch::from_maps(maps)
    }

    fn without_nulls(rows: &[Value]) -> Vec<Value> {
        rows.iter()
            .map(|row| {
                let map = row.as_object().cloned().unwrap_or_default();
                Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).collect())
            })
            .collect()
    }

    fn rows() -> Vec<Value> {
        vec![
            json!({"z": 1, "a": "x", "m": null, "n": [1, {"k": null}], "f": 2.0, "big": u64::MAX}),
            json!({"a": null, "m": null}),
            json!({"z": -5, "q": true, "h": "l1\nl2 é", "o": {"b": 1, "a": 2}}),
        ]
    }

    #[test]
    fn records_are_the_rows_with_sorted_keys_and_no_nulls() {
        let input = rows();
        let b = batch(&input);
        let pretty = serde_json::to_string_pretty(&b.records()).unwrap_or_default();
        let expected = serde_json::to_string_pretty(&without_nulls(&input)).unwrap_or_default();
        assert_eq!(pretty, expected);
    }

    #[test]
    fn a_repeated_column_appears_once_and_a_batch_without_rows_is_an_empty_array() {
        let mut b = batch(&[json!({"a": 1, "b": 2})]);
        b.select_columns(&[1, 0, 1]);
        let text = serde_json::to_string(&b.records()).unwrap_or_default();
        assert_eq!(text, r#"[{"a":1,"b":2}]"#);
        assert_eq!(
            serde_json::to_string(&RecordBatch::empty().records()).unwrap_or_default(),
            "[]"
        );
    }

    #[test]
    fn ndjson_and_json_lines_are_the_compact_records() {
        let input = rows();
        let b = batch(&input);
        let mut ndjson = Vec::new();
        b.write_ndjson(&mut ndjson);
        let lines: Vec<String> = without_nulls(&input).iter().map(Value::to_string).collect();
        assert_eq!(String::from_utf8_lossy(&ndjson), lines.join("\n") + "\n");
        assert_eq!(b.records().json_lines().collect::<Vec<_>>(), lines);
    }

    #[test]
    fn take_limits_across_batches() {
        let first = batch(&[json!({"a": 1}), json!({"a": 2})]);
        let second = batch(&[json!({"a": 3}), json!({"a": 4})]);
        let both = [first, second];
        let records = Records::of(&both).take(3);
        assert_eq!(records.len(), 3);
        assert_eq!(
            serde_json::to_string(&records).unwrap_or_default(),
            r#"[{"a":1},{"a":2},{"a":3}]"#
        );
        assert_eq!(records.json_lines().count(), 3);
    }

    #[test]
    fn merged_keeps_the_columns_a_walk_over_the_kept_rows_meets() {
        let first = batch(&[json!({"b": 1, "a": 2}), json!({"c": null, "a": 3})]);
        let second = batch(&[json!({"d": "x", "b": 4}), json!({"e": 5})]);
        let both = [first, second];
        let names =
            |b: &RecordBatch| -> Vec<String> { b.columns.iter().map(|c| c.to_string()).collect() };
        assert_eq!(names(&RecordBatch::merged(&both, 4)), ["a", "b", "d", "e"]);
        assert_eq!(names(&RecordBatch::merged(&both, 2)), ["a", "b"]);
        let merged = RecordBatch::merged(&both, 3);
        assert_eq!(
            serde_json::to_string(&merged.records()).unwrap_or_default(),
            r#"[{"a":2,"b":1},{"a":3},{"b":4,"d":"x"}]"#
        );
    }
}
