# Sub-kernels

`@sql` runs SQL in an in-process SQLite database and prints the result as a table. `@py` and `@js` send code to a `python3` or `node` worker that stays alive for the session, so a variable set in one command is still there in the next.

## Sub-features

- `sk-sql` runs a SQL query and prints a table.
- `sk-py` runs Python and prints its output.
- `sk-py-state` keeps Python variables between commands.
- `sk-js` runs JavaScript and prints its output.
- `sk-error` reports an exception as a non-zero status without killing the worker.

## How to get to it (user POV)

- `@sql <query>`, `@py '<code>'` and `@js '<code>'` as commands in `koa -c`, the REPL or RPC `exec`.

## Driving it with koa-verify

Preconditions:

- `python3` and `node` are on `PATH`.

- **SQL.** Run `$V c sk-sql '@sql SELECT 1 + 1 AS x'`. `stdout` has an `x` column with value `2`.
- **Python.** Run `$V c sk-py "@py 'print(6*7)'"`. `stdout` is `42`.
- **Python state.** Run `$V c sk-py-state "@py 'x = 41'; @py 'print(x + 1)'"`. `stdout` is `42`.
- **JavaScript.** Run `$V c sk-js "@js 'console.log(6*7)'"`. `stdout` is `42`.
- **Error keeps the worker.** Run `$V c sk-error "@py 'raise ValueError(1)'; echo st=\$?; @py 'print(5)'"`. `stdout` contains `st=1` and then `5`.

## Gotchas

- Worker state lives per session. Separate `koa -c` runs, or separate RPC sessions, do not share variables.
- `@sql SELECT 'a'` loses its single quotes on `main` at the time of writing. Use double-quoted identifiers or numbers in recipes until that fix lands.
- The first `@py` or `@js` call starts the worker, which takes longer than later calls.
