---
name: verify-koa
description: Use to prove a koa behavior on the real binary before calling a change done. Drives koa three ways (`koa -c` one-shots, `koa --rpc` NDJSON sessions, the interactive REPL under a pty) in throwaway directories and saves stdout, stderr, exit codes, RPC events, terminal transcripts and the files left behind as evidence.
---

# Verify koa

koa is a Rust shell with three user surfaces. Users run `koa -c '<script>'` or type into the interactive REPL. Agents talk to `koa --rpc` (newline-delimited JSON over stdio), `koa --socket PATH` (the same protocol on a Unix socket) or `koa --acp`. This skill drives the first three through `scripts/koa-verify`, a Python 3 helper with no dependencies beyond the standard library.

Every command below runs from the repo root. `V=.omp/skills/verify-koa/scripts/koa-verify`.

## Launch

koa is a short-lived process, so there is no server to keep alive. Build once, then each drive starts its own `koa` in its own scratch directory.

```bash
$V build                         # cargo build --release --locked --bin koa
export KOA_VERIFY_RUN=$(date +%H%M%S)-mycheck   # names this run's evidence and scratch dirs
```

Ready means `$V doctor` prints `OK`.

## Doctor

Run it first, and again whenever a result looks wrong.

```bash
$V doctor
```

It prints the repo `HEAD`, whether tracked files have uncommitted changes, `koa --version`, whether the binary is older than any `.rs` or `Cargo.toml` under `crates/` (`binary stale YES` means rebuild), and a probe that runs `koa -c 'echo doctor-ok'`. It exits 1 and prints `NOT OK` when the binary is stale or the probe fails. Never trust a drive on a stale binary: the evidence would describe old code.

## Drive

Each drive creates a fresh `/tmp/koa-verify-$KOA_VERIFY_RUN/<case>/`, writes `--seed NAME=TEXT` files into it (`\n` escapes allowed, subdirectories allowed), runs koa there, and saves evidence.

```bash
# One-shot: stdout, stderr, exit code, files after
$V c <case> '<koa script>' [--seed a.txt='alpha\n' ...]

# RPC: one JSON request per argument, in order. The helper waits for each request's
# reply (`result`, `error`, or the `completed` event) before sending the next.
# $SID is replaced by the session id from the first session.create reply.
$V rpc <case> '{"id":1,"method":"session.create"}' \
              '{"id":2,"method":"exec","params":{"session_id":"$SID","command":"ls"}}'

# Interactive REPL in a pty. Steps: send:TEXT (escapes \r, \x03 Ctrl-C, \x1a Ctrl-Z, \t Tab), sleep:SECS
$V pty <case> 'send:/bin/sleep 30\r' 'sleep:0.5' 'send:\x03' 'send:echo st=$?\r'
```

The feature map in `features/` lists the commands and the proof each feature needs. Start from `features/README.md`.

RPC methods (from `crates/shell-rpc/src/server.rs`): `session.create`, `exec` (`session_id`, `command`, optional `timeout_ms`, `policy`, `format`), `session.fork` (`session_id`, `isolated`), `session.diff`, `session.merge`, `session.snapshot`, `session.restore`, `session.schemas`. `exec` emits `process_spawned`, `stream_chunk` (`stdout` or `stderr`), optional `policy_blocked`, and ends with `completed` (`exit_code`, `duration_ms`, `mutated_files_manifest`, `truncated`).

## Evidence

Everything lands in `target/verify/$KOA_VERIFY_RUN/<case>/`:

- `result.json` always. For `c`: script, `exit_code`, `stdout`, `stderr`. For `rpc`: every request and every reply line, in order; a reply that is not JSON shows up as `{"NON_JSON_LINE": ...}`, which is itself a bug. For `pty`: the steps and `koa_alive_after_steps`.
- `files_after` inside `result.json`: every file left in the scratch dir with its mode and first 200 bytes. This is the side-effect proof for anything that writes, deletes, moves or undoes.
- `transcript.txt` for `pty` drives, with ANSI escapes removed, and `transcript.raw` with them kept.

Proof standards:

- Drive the user path: the same command line a user or agent would type. Do not call Rust functions or test-only hooks.
- Capture the action and the state after it. For any write, `files_after` must show the file contents, not only a success message.
- Assert on exact text. `grep -c 'st=130' target/verify/$KOA_VERIFY_RUN/<case>/transcript.txt` beats reading a transcript by eye.
- For a bug fix, run the same case on a build of `main` first and keep both `result.json` files. The pair is the proof.
- RPC `policy` with `dry_run` is not a sandbox. Check `files_after` to see what really changed.

## Cleanup

```bash
$V cleanup
```

It kills only the koa processes this run started (their PIDs are in `target/verify/$KOA_VERIFY_RUN/pids.txt`, and each is checked to still be this koa binary before it is signalled), removes `/tmp/koa-verify-$KOA_VERIFY_RUN`, and keeps `target/verify/$KOA_VERIFY_RUN`. Never `pkill` or `killall` by name: other koa shells and unrelated processes belong to the user.

## Isolation

Runs with different `KOA_VERIFY_RUN` values never share scratch or evidence directories. Two runs with the same value overwrite each other's cases, so give parallel runs distinct ids. The interactive REPL appends to `$HOME/.koa_history`, so `pty` drives set `HOME` to the case's scratch dir; your real history is untouched, and `~` in a pty drive means the scratch dir.

## Helpers

`scripts/koa-verify` is the only helper. Its usage text is `$V` with no arguments.
