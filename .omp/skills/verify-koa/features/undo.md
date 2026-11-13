# Undo and rollback

koa records the previous contents of files that builtins change, one record per command. `undo` reverts the most recent record, `rollback <tx>` reverts everything from that record onward, and `tx-log` lists the records. After an external command runs, `undo` refuses with `E_TX_OPAQUE` rather than revert past changes koa could not see.

## Sub-features

- `undo-redirect` reverts a `>` write by a builtin.
- `undo-rm` restores a file deleted by the builtin `rm`.
- `undo-opaque` refuses after an external write and keeps the file.
- `undo-rollback` reverts to an earlier record.
- `undo-log` shows records and the opaque marker.

## How to get to it (user POV)

- `undo`, `rollback <tx_id>` and `tx-log` as commands, in `koa -c`, the REPL, or RPC `exec`. Records live in the session, so all steps must run in one `koa -c` script, one REPL, or one RPC session.

## Driving it with koa-verify

Preconditions:

- Seed `a.txt` with `init`.

- **Undo a redirect.** Run `$V c undo-redirect 'echo changed > a.txt; undo' --seed a.txt='init\n'`. `exit_code` is `0` and `files_after` shows `a.txt` as `init\n`.
- **Undo a delete.** Run `$V c undo-rm 'rm a.txt; undo' --seed a.txt='init\n'`. `files_after` shows `a.txt` as `init\n`.
- **Refuse after an external write.** Run `$V c undo-opaque "echo k > a.txt; /bin/sh -c 'echo x > a.txt'; undo" --seed a.txt='init\n'`. `stderr` contains `E_TX_OPAQUE` and `files_after` shows `a.txt` as `x\n`.
- **Rollback.** Run `$V c undo-rollback 'echo one > a.txt; echo two > b.txt; rollback tx_001' --seed a.txt='init\n'`. `files_after` shows `a.txt` as `init\n` and no `b.txt`.
- **Log.** Run `$V c undo-log "echo k > a.txt; /bin/sh -c 'echo x > b.txt'; tx-log" --seed a.txt='init\n'`. `stdout` lists `tx_001` and an `opaque_after` value naming `/bin/sh`.

## Gotchas

- `rollback` is the documented override: it reverts even after an external write. Do not use it to prove `undo` safety.
- Transaction ids count per session from `tx_001`. A fresh `koa -c` starts again at `tx_001`.
- The exhaustive check `scripts/verify-undo.sh` (skill `verify-undo-ledger`) proves the undo promise over every short command sequence. This recipe proves the user path, not the full property.
