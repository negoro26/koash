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
koa -c 'ls | where size > 1000 | sort-by size | select name size'
koa -c 'find src | where ext == rs | first 5'
koa -c 'cat notes.txt | where text =~ todo'
koa -c 'ps | where name == bash | to json'
```

`ls`, `find`, `stat`, `ps`, `cat`, `grep` and `env` return tables. `where`, `select`, `sort-by`, `first`, `last` and `to json` work on those tables.

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

## Aliases and startup file

`alias ll='ls -la'` works as in bash. `alias` lists your aliases, and `unalias ll` removes one.

The interactive shell runs `~/.koarc` when it starts, one line at a time, so you can put `alias`, `export` and `source` lines there. Set `KOA_RC` to use another file, or start with `koa --norc` to skip it. `koa -c` and `koa --rpc` never read it.

`history` lists the commands you typed in the interactive shell.

## For agents

- `koa --rpc` reads one JSON request per line on stdin and answers on stdout.
- `koa --socket PATH` speaks the same protocol on a Unix socket.
- `koa --acp` speaks the Agent Client Protocol.

Each session keeps its own folder, variables and undo history. An `exec` request can set `timeout_ms`, which stops the whole command when the time runs out.

## Bash lines

koa passes lines it does not handle itself to bash. That includes `if`, `for`, `while`, `$(...)`, here-docs, `source`, `.` and `eval`.

After such a line, exported variables, unset variables and the current folder carry over to the next line, so `source .venv/bin/activate` works. Functions, unexported variables and bash aliases defined inside such a line do not carry over.

## Limits

- Wildcards are not expanded yet. `rm *.txt` looks for a file named `*.txt`. Run such lines as `bash -c 'rm *.txt'` for now.
- Ctrl-R does not search history yet.
- `ls` does not support `-t`, `-S` or `-r` yet. Those options run the system `ls` and return plain text.
- Commands hold a whole table in memory before passing it on. They do not stream yet.
- koa's own commands cannot run in the background. `ls &` fails with `E_BG_BUILTIN`.
- The RPC `policy` blocks commands by name only. A full path such as `/usr/bin/touch` gets past it, so it is not a sandbox.
- Undo and the RPC policy do not see inside bash lines.
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
