# Architecture

This document maps the code. Read it before changing the executor or adding a command.
`README.md` covers usage.

## Crates

`shell-builtins` builds on `shell-syntax`, `shell-value`, `shell-kernel`, and `shell-surface`.
`shell-rpc`, `shell-tui`, and `shell-cli` build on `shell-builtins`.

- `shell-syntax` parses a script into a `Program`: list items, pipelines, simple commands, redirections.
  It does not expand words or know any command names.
- `shell-value` defines `Value` (`Empty`, `Bytes`, `Batch`), `RecordBatch`, `StructuredDiagnostic`, Hashline tags, and taint levels.
- `shell-kernel` owns `Session`: cwd, environment, jobs, the kill switch, the tx ledger, and the copy-on-write overlay.
  It also spawns and waits for process groups.
- `shell-builtins` is the executor and every builtin command.
- `shell-surface` renders a `Value` for a terminal, for TSP frames, and in the compact sigil format.
- `shell-rpc` serves NDJSON-RPC and ACP over stdio or a socket. It runs the same executor with a capturing `Sink`.
- `shell-tui` is the line editor, highlighter, and completer.
- `shell-cli` is the `koa` binary. It picks a mode and calls into the crates above.

## Executor flow

`parse` turns the script into a `Program`.
`run_program` walks the list items and applies `&&`, `||`, and `;` against the last status.
`run_pipeline` handles one item.
If the item is a single foreground simple command whose name has `Placement::SoleStage`, it runs through `Builtin::run_alone` and returns.
Otherwise `run_pipeline` opens a tx ledger entry, builds one `Stage` per command, runs the stages, and closes the entry.

`build_stages` expands each word and classifies the stage.

- An assignment-only command sets session variables.
- A name found in the registry with `Placement::Any` is a builtin stage.
- Everything else, including a `SoleStage` name that is not alone, is an external process.

`run_stages` runs the stages in order.
A builtin stage receives the previous value as `Value`, or reads its input first: a stdin redirect, the shell's own stdin when `reads_input` says so, or the bytes of a pipe from an external process.
A builtin produces a `Value` for the next stage.
The last stage hands its value to the `Sink`.
External stages are launched in one process group and joined to their neighbours with pipes.
Output goes to the `Sink`, either the terminal or an RPC channel.
Errors are `StructuredDiagnostic`s. The sink prints them, and `diag_status` maps them to an exit code.
A builtin can also report failures that did not stop it; see Operand failures.

## Registry

Every builtin is a unit struct that implements `Builtin`.
Its `Signature` is the single description of the command: name, aliases, description, flags, output columns, `InputMode`, `Placement`, and `TxPolicy`.
`REGISTRY` in `builtins/mod.rs` lists all commands. `lookup` finds one by name or alias.

Everything else is derived from the signature.

- The executor classifies stages with `lookup` and reads `placement`, `tx`, and `reads_input`.
- `help`, `discover`, and the RPC `session.schemas` method iterate `catalog()`.
  The catalog is the registry without `SoleStage` commands, which are shell control rather than tools.
- Completion and highlighting use `lookup` and the signature's flag spellings and columns.

Invariants:

- A name or alias appears in exactly one signature.
- `catalog()` order is the order of `REGISTRY`. `discover` breaks score ties by that order.
- A `SoleStage` command implements `run_alone` and never runs as a pipeline stage.
- Builtins take `&mut Session`. They do no terminal I/O except through the `Value` they return, apart from `SoleStage` commands, which get the `Sink`.

## Arguments

`Signature::flags` is a list of `Flag`s: a short name, a long name, whether it takes a value, and a help line.
`args::Args::parse` is the only parser. It follows POSIX utility syntax plus GNU long options:
short clusters (`-rf`), `-n5` and `-n 5`, exact long names (`--force`, never a prefix or a substring), `--long=value`, `--` to end options, and a lone `-` as an operand.
An unknown option or a missing value is `E_ARGS`, which `diag_status` maps to exit status 2, before the command touches anything.
`Flag::predicate` declares a single-dash long option (`find -name`) and `Flag::numeric` lets `-5` stand for the flag's value (`head -5`, `kill -9`).
`Args::parse_leading` is the `echo` variant: options only before the first operand, and a dash word it does not know is text.
`help`, `discover`, the RPC `session.schemas` method, and Tab completion render the same list with `flag_spellings`.

## Operand failures

A command that takes many operands (`rm`, `cp`, `mv`, `mkdir`, `cat`, `head`, `tail`, `stat`, `ls`, `grep`, `kill`) does not stop at the first failure.
It calls `Session::report` for each operand that failed and carries on.
After the command returns, the executor prints every report to the sink's stderr and turns the stage status into 1.
A usage error is different: it is returned as `Err`, so it is fatal and exits 2 before any operand is processed.

## Tx ledger

`Session` keeps a bounded list of `TxRecord`s.
`run_pipeline` calls `begin_tx` before the stages and `finish_tx` after them.
A command that changes files calls `record_preimage` or `record_tree` before touching a path and `record_mutation` after.
`finish_tx` drops entries that captured nothing.
`undo` and `rollback <id>` restore preimages newest first.
Commands with `TxPolicy::Skip` (`undo`, `redo`, `rollback`, `tx-log`) do not open an entry, so they do not appear in the ledger they read.
An external process or a sub-kernel command (`@py`, `@js`) can change files the ledger cannot see.
It marks the newest entry `opaque_after`. `undo` then refuses, and `rollback <id>` still works.
With an empty ledger there is no entry to mark, so the session keeps the marker itself until the next transaction captures a path. `undo` refuses with `E_TX_OPAQUE` meanwhile.

### Snapshots

A preimage of a file is a snapshot file on disk, never bytes in memory.
The store is `<start cwd>/.koa/undo/<pid>-<session uid>/`, created on the first snapshot, in the directory that the CoW workspaces already use.
`undo_store::UndoStore` makes each snapshot with a reflink (`FICLONE`), which shares blocks and costs the same for a 1 MB file and a 10 GB file. Where the filesystem cannot clone (tmpfs, another mount), it copies instead.
Only copied bytes count against the 64 MiB per-record limit, and a record holds at most 10,000 paths. Over either limit the record is marked `opaque_after`, so `undo` refuses instead of restoring less than it should.
A `Snapshot` deletes its file when dropped, so evicting a record from the 50-entry ledger frees its snapshots.
The store holds an exclusive `flock` for the life of the session and is removed when the `Session` drops.
`CowWorkspace::sweep_orphans` removes the stores and CoW directories of dead processes. The CLI and the RPC servers call it at startup.
If the store cannot be created, the record is marked `opaque_after`; capture is never skipped silently.

### Redo

`rollback_tx` (and so `undo`) first saves what it is about to overwrite, then restores the preimages.
It keeps the removed records and that saved state as a `RedoRecord` on `Session`'s redo stack.
`redo` restores the saved state and puts the records back on the ledger, oldest first.
A new capture or `mark_opaque` clears the stack, because the saved state would no longer match the disk.
If the saved state does not fit the capture limits, the rollback still happens and `redo` has nothing to re-apply.
`tx-log` reports the number of pending redo records in its `redo` column.

## Adding a command

1. Add `crates/shell-builtins/src/builtins/<name>.rs` with a unit struct, a `static SIGNATURE`, and `impl Builtin`.
2. Add `mod <name>;` and `&<name>::<Struct>` to `REGISTRY` in `builtins/mod.rs`.

Nothing else changes.
Help, discovery, schemas, completion, highlighting, tx handling, and stdin handling follow from the signature.
Put code shared by several commands in `builtins/common.rs`, or in a module of its own when it serves a few of them (`fs_ops.rs` for `rm`, `cp`, `mv`; `slice.rs` for `head`, `tail`).
Declare every option as a `Flag` in the signature and read it through `Args::parse`. Do not write a flag loop.
Override `Builtin::reads_input` when the command reads stdin only without file operands.
