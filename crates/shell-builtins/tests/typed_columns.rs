#![allow(clippy::unwrap_used)]

//! `where` and `sort-by` treat a typed column exactly like a string column holding its text.

use shell_builtins::lookup;
use shell_kernel::Session;
use shell_value::{
    DynRows, FileMode, OrBlank, RecordBatch, Rows, Schema, UnixTime, Value, batch_to_compact_sigil,
    record,
};

record! {
    struct Entry<'a> {
        modified: OrBlank<UnixTime>,
        mode: FileMode,
        name: &'a str,
    }
}

fn typed() -> RecordBatch {
    let times = [
        Some(1_700_000_000),
        None,
        Some(0),
        Some(1_900_000_000),
        Some(253_402_300_800),
        Some(1_700_000_000),
    ];
    let modes = [0o644, 0o7, 0o70, 0o755, 0o777, 0o600];
    let mut rows = Rows::<Entry<'static>>::new();
    for (i, (time, mode)) in times.into_iter().zip(modes).enumerate() {
        rows.push(&Entry {
            modified: OrBlank(time.map(UnixTime)),
            mode: FileMode(mode),
            name: &format!("f{i}"),
        });
    }
    rows.finish().unwrap()
}

fn stringified(batch: &RecordBatch) -> RecordBatch {
    let mut out = DynRows::new(Schema::named(batch.schema().names()));
    for row in batch.rows() {
        out.push_with(|sink| {
            for cell in row.iter() {
                sink.put_str(cell.text(&mut String::new()));
            }
        });
    }
    out.finish().unwrap()
}

fn run(command: &str, args: &[&str], batch: RecordBatch) -> String {
    let mut session = Session::new(false).unwrap();
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let out = lookup(command)
        .unwrap()
        .run(&mut session, &args, Value::from_batch(batch))
        .unwrap();
    batch_to_compact_sigil(&out.into_batch().unwrap())
}

#[test]
fn where_and_sort_by_agree_between_a_typed_column_and_its_text() {
    let a = typed();
    let b = stringified(&a);
    let cases: [(&str, &[&str]); 12] = [
        ("where", &["modified", ">", "2023"]),
        ("where", &["modified", "<", "2023-11"]),
        ("where", &["modified", "==", ""]),
        ("where", &["modified", "contains", "1970"]),
        ("where", &["modified", ">=", "2023"]),
        ("where", &["mode", "==", "644"]),
        ("where", &["mode", ">", "7"]),
        ("where", &["mode", "contains", "7"]),
        ("sort-by", &["modified"]),
        ("sort-by", &["--desc", "modified"]),
        ("sort-by", &["mode"]),
        ("sort-by", &["--desc", "mode"]),
    ];
    for (command, args) in cases {
        assert_eq!(
            run(command, args, a.clone()),
            run(command, args, b.clone()),
            "{command} {args:?}"
        );
    }
}
