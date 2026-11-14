#![allow(clippy::unwrap_used)]

//! A typed column must behave exactly like a string column that holds the same text.

use shell_value::{
    Datum, DynRows, FileMode, OrBlank, RecordBatch, Rows, Schema, UnixTime, batch_to_compact_sigil,
    record, render_batch_table,
};

record! {
    struct Entry<'a> {
        modified: OrBlank<UnixTime>,
        mode: FileMode,
        name: &'a str,
    }
}

const TIMES: [Option<u64>; 8] = [
    Some(0),
    Some(86_399),
    Some(951_782_400),
    Some(1_700_000_000),
    Some(253_402_300_800),
    Some(1 << 40),
    None,
    Some(5),
];
const MODES: [u32; 8] = [0, 7, 0o70, 0o644, 0o777, 0o4755, 0o100, 0o10];

fn typed() -> RecordBatch {
    let mut rows = Rows::<Entry<'static>>::new();
    for (i, (time, mode)) in TIMES.iter().zip(MODES).enumerate() {
        let name = format!("n{}", 7 - i);
        rows.push(&Entry {
            modified: OrBlank(time.map(UnixTime)),
            mode: FileMode(mode),
            name: &name,
        });
    }
    rows.finish().unwrap()
}

fn stringified(batch: &RecordBatch) -> RecordBatch {
    let mut out = DynRows::new(Schema::named(batch.schema().names()));
    let mut scratch = String::new();
    for row in batch.rows() {
        out.push_with(|sink| {
            for cell in row.iter() {
                if cell.is_null() {
                    sink.put(Datum::Null);
                } else {
                    sink.put_str(cell.text(&mut scratch));
                }
            }
        });
    }
    out.finish().unwrap()
}

fn encodings(batch: &RecordBatch) -> Vec<String> {
    let mut table = Vec::new();
    render_batch_table(batch, &mut table, false);
    let mut colored = Vec::new();
    render_batch_table(batch, &mut colored, true);
    let mut ndjson = Vec::new();
    batch.write_ndjson(&mut ndjson);
    let both = [batch.clone(), batch.clone()];
    vec![
        String::from_utf8_lossy(&table).into_owned(),
        String::from_utf8_lossy(&colored).into_owned(),
        String::from_utf8_lossy(&ndjson).into_owned(),
        batch_to_compact_sigil(batch),
        serde_json::to_string_pretty(&batch.records()).unwrap(),
        batch.records().json_lines().collect::<Vec<_>>().join("\n"),
        batch_to_compact_sigil(&RecordBatch::merged(&both, 11)),
    ]
}

#[test]
fn every_encoder_prints_a_typed_column_as_its_text() {
    let typed = typed();
    assert_eq!(encodings(&typed), encodings(&stringified(&typed)));
}

#[test]
fn a_typed_cell_reads_as_text_and_never_as_a_number() {
    let typed = typed();
    let plain = stringified(&typed);
    let mut scratch = String::new();
    for (t, p) in typed.rows().zip(plain.rows()) {
        for (a, b) in t.iter().zip(p.iter()) {
            assert_eq!(a.as_f64(), None);
            assert_eq!(a.is_null(), b.is_null());
            assert_eq!(a.text(&mut scratch).to_string(), b.text(&mut String::new()));
            assert_eq!(a.text_len(&mut scratch), b.text_len(&mut String::new()));
            let mut written = Vec::new();
            a.write_text(&mut written, &mut scratch);
            assert_eq!(written, b.text(&mut String::new()).as_bytes());
        }
    }
}

#[test]
fn reordering_and_selecting_keep_the_typing() {
    let mut a = typed();
    let mut b = stringified(&a);
    for batch in [&mut a, &mut b] {
        let name = batch.col_index("name").unwrap();
        let order = batch.order_by(|row| row.get(name).as_str().unwrap_or("").to_string());
        batch.take_rows(order);
        batch.retain_rows(|row| row.get(name).as_str() != Some("n3"));
        let modified = batch.col_index("modified").unwrap();
        batch.select_columns(&[modified, name, modified]);
    }
    assert_eq!(encodings(&a), encodings(&b));
}

#[test]
fn a_plain_number_written_into_a_typed_column_is_stored_as_text() {
    let mut rows = DynRows::new(Schema::of(<Entry<'static> as shell_value::Record>::COLUMNS));
    rows.push_with(|out| {
        out.put(Datum::UInt(5));
        out.put(Datum::Time(UnixTime(5)));
        out.put(Datum::Null);
    });
    let batch = rows.finish().unwrap();
    let row = batch.row(0);
    assert_eq!(row.get(0), Datum::Str("5"));
    assert_eq!(row.get(1), Datum::Str("1970-01-01T00:00:05Z"));
    assert_eq!(row.get(2), Datum::Null);
}
