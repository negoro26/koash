# koash

`koa` is a command shell for people and AI agents, written in Rust. Its own commands pass tables instead of text, every file change they make can be undone, and agents can drive it over JSON.

It runs on Linux.

## Install

Download a binary for x86_64 or aarch64 from [Releases](https://github.com/negoro26/koash/releases), or build it:

```bash
cargo install --git https://github.com/negoro26/koash
```

## Use

Run `koa` for an interactive shell, or `koa -c '...'` to run one line.

```bash
koa -c 'ls -t | first 5'
koa -c 'ls | where size > 1000 | sort-by size | select name size'
koa -c 'find src | where ext == rs | first 5'
koa -c 'cat notes.txt | where text =~ todo'
koa -c 'ps | where name == bash | to json'
```

`ls`, `find`, `stat`, `ps`, `cat`, `grep` and `env` return tables. `where`, `select`, `sort-by`, `first`, `last` and `to json` work on those tables.

A bare `ls` on a terminal looks like `eza -lh --group-directories-first --icons=auto`. It has the same columns, `24k` sizes, owner names, `12 Nov 21:36` dates, Nerd Font icons, colours and symlink targets, and folders come first:

```text
Permissions Size User Date Modified Name
drwxr-xr-x     - alex 12 Nov 21:36  src
.rw-r--r--  1.5k alex 12 Nov 21:36  Cargo.toml
lrwxrwxrwx     - alex 12 Nov 21:36  latest -> Cargo.toml
```

`ls -1` prints one name per line in the same colours. `ls -t`, `-S`, `-r` and `-a` still sort and filter, and folders still come first. koa reads `LS_COLORS`, `EZA_COLORS`, `NO_COLOR`, `EZA_ICON_SPACING` and `COLUMNS` the way eza does. Icons show only when the terminal reports a width.

Everything else about `ls` is unchanged. `ls | where size > 1000`, `ls | to json`, `ls > files.json`, `ls | wc -l` and RPC requests still get the table, with the same columns and the same row order.

Normal programs still work. When a table goes into one, it gets the text bash would send:

```bash
koa -c 'ls | wc -l'
koa -c 'find . -name "*.log" | xargs rm'
```

`grep` is koa's own command and searches lines of text. To filter file names, use `where`:

```bash
koa -c 'ls | where name =~ test'
```

## Undo

koa saves a copy of a file before its own commands change it. That covers `rm`, `cp`, `mv`, `mkdir`, `patch`, `rename` and `>` or `>>` redirects.

```bash
koa -c 'rm notes.txt; undo'
```

`undo` puts back the last change, `redo` reapplies it, and `tx-log` lists the changes it can undo. Once a normal program has run, `undo` refuses with `E_TX_OPAQUE`, because koa cannot see what that program changed.

## Safe edits

`cat` prints a short hash next to each line. `patch` changes a line only if it still has that hash, so an edit based on an old read fails instead of overwriting newer text.

```bash
koa -c 'cat main.rs'
koa -c 'patch main.rs 12:4c7 "let x = 2;"'
```

## Other commands

- `@sql SELECT 1 + 1` runs SQL in an SQLite database that lives as long as the session.
- `@py` and `@js` send code to a Python or Node process that stays running, so variables persist between lines.
- `refs NAME` finds every line that uses `NAME` as a whole word.
- `discover "undo a change"` suggests commands for a task.
- `help` lists every command, its options and its columns.

## For agents

- `koa --rpc` reads one JSON request per line on stdin and answers on stdout.
- `koa --socket PATH` speaks the same protocol on a Unix socket.
- `koa --acp` speaks the Agent Client Protocol.

Each session keeps its own folder, variables and undo history. An `exec` request can set `timeout_ms`, which stops the whole command when the time runs out.

## Bash lines

koa passes lines it does not handle itself to bash. That includes `if`, `for`, `while`, `$(...)`, here-docs, `source`, `.` and `eval`.

After such a line, exported variables, unset variables and the current folder carry over to the next line, so `source .venv/bin/activate` works. Functions, aliases and unexported variables do not carry over.

## Limits

- Wildcards are not expanded yet. `rm *.txt` looks for a file named `*.txt`. Run such lines as `bash -c 'rm *.txt'` for now.
- There is no `alias`, startup file or `history` command yet, and Ctrl-R does not search history.
- Commands hold a whole table in memory before passing it on. They do not stream yet.
- koa's own commands cannot run in the background. `ls &` fails with `E_BG_BUILTIN`.
- The RPC `policy` blocks commands by name only. A full path such as `/usr/bin/touch` gets past it, so it is not a sandbox.
- Undo and the RPC policy do not see inside bash lines.
- The eza look of `ls` uses eza's default date format and English month names. It does not read `TIME_STYLE`, `LC_TIME` or an eza theme file. Under Tern (`TERM_PROGRAM=Tern`) and `--tsp`, `ls` still sends the table frame.
- macOS is untested.

## Development

`scripts/check.sh` runs everything CI runs: formatting, lints, tests, and three checks that compare the built `koa` with Python and GNU coreutils.

```bash
scripts/check.sh
scripts/verify-undo.sh       # every sequence of up to 5 commands against the undo rules
scripts/bench.sh BINARY      # times ls, pipelines and cat on large inputs
scripts/profile.sh flamegraph -- -c 'find . | where ext == rs'
```

Code comments are kept for reasons the code cannot show. `scripts/check.sh` fails if the number of comment lines goes up. `docs/ARCHITECTURE.md` explains how the code fits together and how to add a command.

## License

MIT
