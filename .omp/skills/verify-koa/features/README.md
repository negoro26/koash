# koa verification map

This directory is the maintained source for verifying what koa users and agents can observe. Read this index, then use the matching feature file as the recipe.

## Baseline preconditions

- `$V build` succeeded and `$V doctor` prints `OK` (`V=.omp/skills/verify-koa/scripts/koa-verify`, run from the repo root).
- `KOA_VERIFY_RUN` is set to an id unique to this run.
- `python3` and `node` are on `PATH` for the sub-kernel recipes.
- Never drive a koa process this run did not start.

## Driving conventions

- Every case starts in an empty scratch dir. Seed files with `--seed`; never point a case at a real project directory, because several recipes delete and move files.
- Use absolute paths for externals (`/bin/cat`, `/bin/sh`) so `PATH` differences cannot change which program runs.
- Quote koa scripts in single quotes on the host shell so `$?` and `$VAR` reach koa unexpanded.
- In `pty` steps, end a command with `\r`, not `\n`.

## Proof and skip reporting

- One-shot proof is the script, `exit_code`, `stdout`, `stderr` and `files_after` from `result.json`.
- RPC proof is the ordered reply list in `result.json`, including the `completed` event and its `exit_code`.
- Interactive proof is a string asserted in `transcript.txt` with `grep -c`.
- For a behavior change, keep the same case run against a `main` build next to the branch result.
- Report a recipe you could not run with the command you tried and the precondition that failed. Do not report a feature as verified through a different entry point than the one listed.

## Feature entry contract

Each feature file starts with an H1 and one paragraph of user-visible behavior, then four H2s in order: `Sub-features`, `How to get to it (user POV)`, `Driving it with koa-verify`, `Gotchas`.

## Features

- [Structured pipelines](./structured-pipelines.md) covers tables from `ls`, `cat` and `grep`, the `where`/`select`/`sort-by`/`first`/`to` verbs, and the hand-off to and from external commands.
- [Undo and rollback](./undo.md) covers recorded builtin writes, `undo`, `rollback`, `tx-log` and the `E_TX_OPAQUE` refusal after an external command.
- [RPC sessions](./rpc.md) covers `session.create`, `exec` output events, exit codes, `timeout_ms` and output isolation.
- [Interactive job control](./job-control.md) covers Ctrl-C, Ctrl-Z, `jobs`, `fg` and Tab completion in the REPL.
- [Sub-kernels](./sub-kernels.md) covers `@sql`, and `@py`/`@js` state that survives between commands.
