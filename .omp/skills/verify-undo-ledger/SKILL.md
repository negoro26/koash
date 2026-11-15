---
name: verify-undo-ledger
description: Use before merging any change to koa's undo/rollback ledger, the opaque marker, or anything that runs external commands or writes files on a builtin's behalf. Checks every command sequence up to depth 5 against the real code to prove `undo` never overwrites an external command's work and always restores exactly the state before the undone transaction.
---

# Verify the undo ledger

## What is promised

- **P1.** A successful `undo` never changes a file whose current contents were written by an external command. If this breaks, `undo` silently destroys work koa cannot restore.
- **P2.** A successful `undo` of transaction `T` puts every path `T` captured back to its contents just before `T` ran, and leaves every other path alone. If this breaks, `undo` reports success while leaving files in a state the user never had.

`rollback <id>` is the documented override and may overwrite external work, so neither property applies to it. Sequences that contain `rollback` are still checked, because a rollback must leave the next `undo` safe.

## How it is checked

`crates/shell-builtins/tests/undo_ledger_exhaustive.rs` drives the real `shell_builtins::run_program` on a real `Session` in a scratch directory. It enumerates every sequence, from length 1 up to the depth, over 9 commands on two files, `a` (starts as `init`) and `b` (starts absent):

| Command | Stands for |
|---|---|
| `echo kN > a`, `echo kN > b` | builtin write through a redirect, captured by the ledger |
| `rm a`, `rm b` | builtin delete; it fails when the file is absent, which covers the empty record a failed builtin leaves |
| `/bin/sh -c 'echo xN > a'`, `… > b` | external write the ledger cannot capture |
| `undo` | the command under test |
| `rollback <oldest id>`, `rollback <newest id>` | override that drains records |

After each command the harness reads both files, records which kind of command last changed each one, and checks P1 and P2 after every `undo` that exits 0. Every step runs the shipped code: parser, executor, process spawn, and ledger. Nothing is modeled.

Depth 5 covers 66,429 sequences in about 2 minutes on 4 cores (111 s measured). Depth 4 covers 7,380 in about 10 seconds (9.4 s measured), plus compile time.

## When to run it

Run it when a diff touches any of these:

| File | Functions |
|---|---|
| `crates/shell-kernel/src/session.rs` | `begin_tx`, `record_preimage`, `record_tree`, `capture_one`, `record_output_target`, `mark_capture_failed`, `mark_opaque`, `finish_tx`, `rollback_tx`, `undo_last`, `TxRecord`, `Preimage` |
| `crates/shell-builtins/src/pipeline.rs` | `run_pipeline` (its `finish_tx` call), `begin_pipeline_tx`, `finish_builtin_output` |
| `crates/shell-builtins/src/wiring.rs` | `launch_external` (the `mark_opaque` after `spawn_external`), `open_for_write` |
| `crates/shell-builtins/src/builtins/fs.rs` | `rm`, `mkdir`, `cp`, `mv` capture calls |
| `crates/shell-builtins/src/builtins/tx.rs` | `undo`, `rollback` |
| `crates/shell-builtins/src/builtins/mod.rs` | the `@py`/`@js` `mark_opaque` calls |

Sync commit: `f3e37b2`. At that commit the harness was confirmed to drive the code above, and the check passed at depth 5.

To see whether covered code changed since the sync commit:

```bash
git diff --stat f3e37b2 -- crates/shell-kernel/src/session.rs crates/shell-builtins/src/pipeline.rs crates/shell-builtins/src/wiring.rs crates/shell-builtins/src/builtins/fs.rs crates/shell-builtins/src/builtins/tx.rs crates/shell-builtins/src/builtins/mod.rs
```

## Run it

```bash
scripts/verify-undo.sh       # depth 5, run before merging a covered change
scripts/verify-undo.sh 4     # quick pass while iterating
```

A pass prints `checked 66429 command sequences (depth 5, 9 commands, files a/b)` and `test result: ok`. The normal test suite runs the same test at depth 3, so gross breaks also fail `cargo nextest run`.

## Read a failure

A failure panics with the property, a one-line reason, and the shortest failing sequence. Each step shows the exact koa command, its exit status and error code, both files, and the ledger with any `opaque_after` markers:

```
P1: a successful undo never overwrites a file an external command wrote
`undo` changed `a`, last written by an external command
Start: a="init" b=<absent>
0. `echo k0 > a` -> status 0 | files: a="k0" b=<absent> | ledger: [tx_001]
1. `echo k1 > b` -> status 0 | files: a="k0" b="k1" | ledger: [tx_001, tx_002]
2. `/bin/sh -c 'echo x2 > a'` -> status 0 | files: a="x2" b="k1" | ledger: [tx_001, tx_002 opaque_after="/bin/sh"]
3. `rollback tx_002` -> status 0 | files: a="x2" b=<absent> | ledger: [tx_001]
4. `undo` -> status 0 | files: a="init" b=<absent> | ledger: []
```

To tell a real bug from a harness error, replay the sequence by hand with the release binary in an empty directory that holds only `a` containing `init`:

```bash
koa -c 'echo k0 > a; echo k1 > b; /bin/sh -c "echo x2 > a"; rollback tx_002; undo'; cat a
```

If the hand replay shows the same file contents, the code is wrong. Fix the code, not the property. If the replay differs, the harness is wrong. Check its file snapshots, its tx-id bookkeeping (`before_tx`), and its idea of who last wrote each file (`author`), then fix the harness.

## Change the verification on purpose

- **Intended behavior change** (say `undo` should start skipping opaque records). Update P1 or P2 in this file and the matching check in `run_sequence`. Then break the new rule on purpose in `session.rs` and confirm the check fails before you trust a pass.
- **New command that writes files.** If it is a builtin, add a write variant to `Op` and `ALPHABET`. If it is an external path (a new sub-kernel, for example), add it as an external-author command. Each new command multiplies the run time, so measure depth 5 again and update the numbers above.
- **After you confirm** that the harness drives the code at a new commit and the check passes at depth 5, move the sync commit above forward to that commit. Do not move it on a pass alone. First read the covered functions and confirm that the commands still reach them.

## Not covered

- More than 5 commands, more than two files, directories, symlinks, file modes, and contents larger than a line.
- The 50-record ledger cap in `begin_tx`, which needs over 50 transactions.
- The capture byte and entry limits (`TX_CAPTURE_MAX_BYTES`, `TX_CAPTURE_MAX_ENTRIES`).
- `mv`, `cp`, `mkdir`, `patch`, `rename`, and `@py`/`@js`. They share the capture and opaque paths but are not commands in the sequences.
- Concurrent sessions, RPC slot handling, and anything that writes to the directory outside koa between commands.
- A pass covers these sequences on this filesystem. It does not prove the property for every sequence.
