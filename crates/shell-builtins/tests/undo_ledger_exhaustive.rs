#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};

use shell_builtins::{Chunk, Sink, run_program};
use shell_kernel::Session;

const FILES: [&str; 2] = ["a", "b"];

#[derive(Clone, Copy, Debug)]
enum Op {
    BuiltinWrite(usize),
    BuiltinRm(usize),
    ExternalWrite(usize),
    Undo,
    RollbackOldest,
    RollbackNewest,
}

const ALPHABET: [Op; 9] = [
    Op::BuiltinWrite(0),
    Op::BuiltinWrite(1),
    Op::BuiltinRm(0),
    Op::BuiltinRm(1),
    Op::ExternalWrite(0),
    Op::ExternalWrite(1),
    Op::Undo,
    Op::RollbackOldest,
    Op::RollbackNewest,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Author {
    Initial,
    Koa,
    External,
}

type Snapshot = BTreeMap<&'static str, Option<Vec<u8>>>;

fn snapshot(dir: &Path) -> Snapshot {
    FILES
        .iter()
        .map(|f| (*f, std::fs::read(dir.join(f)).ok()))
        .collect()
}

fn show(s: &Snapshot) -> String {
    s.iter()
        .map(|(f, c)| match c {
            Some(bytes) => format!("{f}={:?}", String::from_utf8_lossy(bytes).trim_end()),
            None => format!("{f}=<absent>"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_text(op: Op, step: usize, session: &Session) -> String {
    match op {
        Op::BuiltinWrite(f) => format!("echo k{step} > {}", FILES[f]),
        Op::BuiltinRm(f) => format!("rm {}", FILES[f]),
        Op::ExternalWrite(f) => format!("/bin/sh -c 'echo x{step} > {}'", FILES[f]),
        Op::Undo => "undo".to_string(),
        Op::RollbackOldest => format!(
            "rollback {}",
            session
                .tx_log
                .first()
                .map_or("tx_000".to_string(), |t| t.id.clone())
        ),
        Op::RollbackNewest => format!(
            "rollback {}",
            session
                .tx_log
                .last()
                .map_or("tx_000".to_string(), |t| t.id.clone())
        ),
    }
}

struct Violation {
    property: &'static str,
    detail: String,
    trace: Vec<String>,
}

fn run_sequence(dir: &Path, ops: &[Op]) -> Result<(), Violation> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("scratch dir");
    std::fs::write(dir.join("a"), "init\n").expect("seed a");

    let mut session = Session::new(false).expect("session");
    session.cwd = dir.to_path_buf();
    let (tx, rx) = sync_channel::<Chunk>(4096);
    let sink = Sink::Capture(tx);

    let mut author: BTreeMap<&'static str, Author> =
        FILES.iter().map(|f| (*f, Author::Initial)).collect();
    let mut before_tx: BTreeMap<String, Snapshot> = BTreeMap::new();
    let mut trace = Vec::new();

    for (step, op) in ops.iter().copied().enumerate() {
        let text = command_text(op, step, &session);
        let before = snapshot(dir);
        let top_before = session.tx_log.last().map(|t| t.id.clone());
        let undo_target = session.tx_log.last().map(|t| {
            (
                t.id.clone(),
                t.preimages
                    .iter()
                    .map(|(p, _)| p.clone())
                    .collect::<Vec<PathBuf>>(),
            )
        });

        let program = shell_syntax::parse(&text).expect("parse");
        let status = run_program(&mut session, &program, &sink);
        let diags: Vec<String> = rx
            .try_iter()
            .filter_map(|c| match c {
                Chunk::Diag(d) => Some(d.code.clone()),
                _ => None,
            })
            .collect();

        let after = snapshot(dir);
        if let Some(top) = session.tx_log.last()
            && Some(&top.id) != top_before.as_ref()
            && !before_tx.contains_key(&top.id)
        {
            before_tx.insert(top.id.clone(), before.clone());
        }

        trace.push(format!(
            "{step}. `{text}` -> status {status}{} | files: {} | ledger: [{}]",
            if diags.is_empty() {
                String::new()
            } else {
                format!(" {}", diags.join(","))
            },
            show(&after),
            session
                .tx_log
                .iter()
                .map(|t| match &t.opaque_after {
                    Some(o) => format!("{} opaque_after={o:?}", t.id),
                    None => t.id.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));

        let changed: Vec<&'static str> = FILES
            .iter()
            .copied()
            .filter(|f| before[f] != after[f])
            .collect();

        if matches!(op, Op::Undo) && status == 0 {
            for f in &changed {
                if author[f] == Author::External {
                    return Err(Violation {
                        property: "P1: a successful undo never overwrites a file an external command wrote",
                        detail: format!(
                            "`undo` changed `{f}`, last written by an external command"
                        ),
                        trace,
                    });
                }
            }
            if let Some((id, paths)) = undo_target {
                let Some(pre) = before_tx.get(&id) else {
                    return Err(Violation {
                        property: "P2: a successful undo restores exactly the state before the undone transaction",
                        detail: format!("`undo` reverted {id}, which the checker never saw begin"),
                        trace,
                    });
                };
                for f in FILES {
                    let captured = paths.iter().any(|p| p == &dir.join(f));
                    let expected = if captured { &pre[f] } else { &before[f] };
                    if &after[f] != expected {
                        return Err(Violation {
                            property: "P2: a successful undo restores exactly the state before the undone transaction",
                            detail: format!(
                                "after undoing {id}, `{f}` is {:?}, expected {:?}",
                                after[f].as_deref().map(String::from_utf8_lossy),
                                expected.as_deref().map(String::from_utf8_lossy),
                            ),
                            trace,
                        });
                    }
                }
            } else if !changed.is_empty() {
                return Err(Violation {
                    property: "P2: a successful undo restores exactly the state before the undone transaction",
                    detail: "`undo` changed files with an empty ledger".to_string(),
                    trace,
                });
            }
        }

        let who = match op {
            Op::ExternalWrite(_) => Author::External,
            _ => Author::Koa,
        };
        for f in changed {
            author.insert(f, who);
        }
    }
    Ok(())
}

fn sequences(depth: usize) -> impl Iterator<Item = Vec<Op>> {
    (1..=depth).flat_map(|len| {
        let total = ALPHABET.len().pow(len as u32);
        (0..total).map(move |mut n| {
            let mut ops = Vec::with_capacity(len);
            for _ in 0..len {
                ops.push(ALPHABET[n % ALPHABET.len()]);
                n /= ALPHABET.len();
            }
            ops
        })
    })
}

#[test]
fn undo_is_safe_for_every_command_sequence_up_to_depth() {
    let depth: usize = std::env::var("KOA_UNDO_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let all: Vec<Vec<Op>> = sequences(depth).collect();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = Arc::new(Mutex::new(0usize));
    let found: Arc<Mutex<Option<(usize, Violation)>>> = Arc::new(Mutex::new(None));

    std::thread::scope(|scope| {
        for w in 0..workers {
            let all = &all;
            let next = Arc::clone(&next);
            let found = Arc::clone(&found);
            scope.spawn(move || {
                let dir = std::env::temp_dir()
                    .join(format!("koa-undo-verify-{}-{w}", std::process::id()));
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        let i = *n;
                        *n += 1;
                        i
                    };
                    if i >= all.len() || found.lock().unwrap().as_ref().is_some_and(|(j, _)| *j < i)
                    {
                        break;
                    }
                    if let Err(v) = run_sequence(&dir, &all[i]) {
                        let mut slot = found.lock().unwrap();
                        if slot.as_ref().is_none_or(|(j, _)| i < *j) {
                            *slot = Some((i, v));
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(&dir);
            });
        }
    });

    if let Some((_, v)) = found.lock().unwrap().take() {
        panic!(
            "\n{}\n{}\nStart: a=\"init\" b=<absent>\n{}\n",
            v.property,
            v.detail,
            v.trace.join("\n")
        );
    }
    eprintln!(
        "checked {} command sequences (depth {depth}, {} commands, files a/b)",
        all.len(),
        ALPHABET.len()
    );
}
