# koash

`koa` is a command shell for people and AI agents, written in Rust. Its own commands pass tables instead of text, every file change they make can be undone, and agents can drive it over JSON.

It runs on Linux.

## Install

Download a binary for x86_64 or aarch64 from [Releases](https://github.com/negoro26/koash/releases), or build it:

```bash
cargo install --git https://github.com/negoro26/koash
```

## Use

Run `koa` for an interactive shell, or `koa -c '...'` to run one line. In the interactive shell, Up and Down step through history, Ctrl-R searches it, and Tab completes commands and paths. The usual readline keys work: Ctrl-A, Ctrl-E, Ctrl-U, Ctrl-K, Ctrl-W, Alt-B and Alt-F.

```bash
koa -c 'ls -t | first 5'
koa -c 'ls | where size > 1000 | sort-by size | select name size'
koa -c 'find src | where ext == rs | first 5'
koa -c 'cat notes.txt | where text =~ todo'
koa -c 'ps | where name == bash | to json'
```

`ls`, `find`, `stat`, `ps`, `cat`, `grep` and `env` return tables. On a terminal without a pipe or redirect, `ls` draws a native GNU-style view (multi-column `-C` fitting the terminal width, `-1`, `-l`, `-lh`, `LS_COLORS`, `NO_COLOR`, and symlink targets), while pipes, redirects, `to json`, TSP and RPC still get the full structured table. `where`, `select`, `sort-by`, `first`, `last` and `to json` work on those tables.

Normal programs still work. When a table goes into one, it gets the text bash would send:

```bash
koa -c 'ls | wc -l'
koa -c 'find . -name "*.log" | xargs rm'
```

Wildcards and braces expand as in bash: `rm *.log`, `cat src/*.rs`, `mkdir -p app/{src,test}`. A pattern that matches nothing stays as typed.

`grep` works on tables too. It matches the name for `ls`, the path for `find` and the line for `cat`, and keeps the whole row:

```bash
koa -c 'ls | grep -i test | select name size'
koa -c 'find . | grep -v target'
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

## Background jobs

End a line with `&` to run it in the background. `jobs` lists the jobs, `fg` and `bg` bring one to the front or resume it, `kill %1` ends job 1, `wait` waits for them and Ctrl-Z stops the job in front. `$!` is the process id of the last background job, and `wait $!` or `wait %1` returns its exit status.

```bash
koa -c 'find . | where size > 1000000 | select path > big.txt &'
```

koa's own commands live inside the shell, so a background pipeline that uses one runs in a child `koa -c`. Like `&` in bash, variable and folder changes made there do not come back. `undo` refuses afterwards, as it does after any normal program. Output goes to the terminal, and over RPC it is dropped. Ctrl-Z cannot stop a koa command that runs in front, so start it with `&` if you may want it back.

## Other commands

- `@sql SELECT 1 + 1` runs SQL in an SQLite database that lives as long as the session.
- `@py` and `@js` send code to a Python or Node process that stays running, so variables persist between lines.
- `refs NAME` finds every line that uses `NAME` as a whole word.
- `discover "undo a change"` suggests commands for a task.
- `help` lists every command and its columns. `help ls` shows one command in full, with every option.

On a terminal a table never runs past the screen. The widest columns are cut first and a cut cell ends in `…`. koa reads the width from the terminal, or from `COLUMNS` when that is set. A table with one row that does not fit is printed as one `name  value` line per column, wrapped. Pipes, files, `to json` and RPC get the full table.

## Aliases and startup file

`alias ll='ls -la'` works as in bash, including an alias whose value ends in a space expanding the next word. `alias` lists your aliases, and `unalias ll` removes one. Aliases are not expanded inside `$(...)` or backticks, and Tab completion does not know alias names yet.

The interactive shell runs `~/.koarc` when it starts. Set `KOA_RC` to use another file, or start with `koa --norc` to skip it. `koa -c`, piped scripts, `--rpc`, `--socket` and `--acp` never read it.

koa runs the file one line at a time, so put each `alias`, `export` or `source` on its own line. A multi-line `if` or function does not work there; write it on one line, or put it in a file and `source` it. A koa syntax error in the file prints `.koarc:LINE:` and the rest of the file still runs. Other errors print without the line number.

`history` lists the commands you typed in the interactive shell, and `history 5` the last five. There is no `history -c` yet.

## For agents

- `koa --rpc` reads one JSON request per line on stdin and answers on stdout.
- `koa --socket PATH` speaks the same protocol on a Unix socket.
- `koa --acp` speaks the Agent Client Protocol.

Each session keeps its own folder, variables and undo history. An `exec` request can set `timeout_ms`, which stops the whole command when the time runs out.

## Bash lines

koa passes lines it does not handle itself to bash. That includes `if`, `for`, `while`, `$(...)`, here-docs, `source`, `.` and `eval`.

After such a line, exported variables, unset variables and the current folder carry over to the next line, so `source .venv/bin/activate` works. Functions, unexported variables and bash aliases defined inside such a line do not carry over.

## Limits

- Commands hold a whole table in memory before passing it on. They do not stream yet.
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
