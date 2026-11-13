# Structured pipelines

Builtins such as `ls`, `cat` and `grep` produce tables. Table verbs filter and reshape them in-process, and the final table prints as an aligned text table. When a table reaches an external command it is written as text, and external output comes back as one row per line.

## Sub-features

- `pipe-table` prints a table from a builtin.
- `pipe-filter` narrows rows with `where` and columns with `select`.
- `pipe-order` sorts with `sort-by` and limits with `first`/`last`.
- `pipe-json` converts with `to json`.
- `pipe-external` passes a table through an external command and back.
- `pipe-error` turns a failing builtin into a status that `||` can recover from.

## How to get to it (user POV)

- `koa -c '<pipeline>'` from a terminal.
- The same pipeline typed at the `koa>` prompt.
- RPC `exec` with the pipeline as `command`.

## Driving it with koa-verify

Preconditions:

- Seed `a.txt`, `b.txt` and `sub/c.txt` with the `--seed` flags shown.

- **Table from a builtin.** Run `$V c pipe-table 'ls | select name kind' --seed a.txt='alpha\n' --seed b.txt='beta\n' --seed sub/c.txt='c\n'`. `exit_code` is `0` and `stdout` has a `name  kind` header with rows `a.txt  file`, `b.txt  file` and `sub    dir`.
- **Filter and order.** Run `$V c pipe-filter 'ls | where kind == file | sort-by name | first 1 | select name size' --seed a.txt='alpha\n' --seed b.txt='beta\n'`. `stdout` has exactly one data row, `a.txt  6`.
- **JSON out.** Run `$V c pipe-json 'cat a.txt | to json' --seed a.txt='alpha\n'`. `stdout` parses as a JSON array whose single object has `"text": "alpha"` and `"line": 1`.
- **Through an external and back.** Run `$V c pipe-external 'ls | select name | /usr/bin/sort -r | first 1' --seed a.txt='a\n' --seed b.txt='b\n'`. `stdout` is a one-row `name` table with `b.txt`.
- **Large input does not hang.** Run `$V c pipe-large '/usr/bin/seq 1 50000 > big.txt; cat big.txt | /bin/cat | first 1 | select line text'`. `exit_code` is `0` and `stdout` is a one-row table with `line` `1` and `text` `1`, never `TIMEOUT`.
- **Error becomes a status.** Run `$V c pipe-error 'cat nope.txt || echo recovered; cat nope.txt; echo st=$?'`. `stdout` contains `recovered` and `st=1`, and `stderr` contains `E_CAT`.

## Gotchas

- Column names come from the builtin. Run the builtin alone first to see its columns before writing a `where` against them.
- `where` with `>=` or `<=` is a known bug on `main` at the time of writing: it returns an empty table with exit code `0`. Assert on row counts, never only on the exit code.
- `size` is in bytes and includes the trailing newline of seeded files.
- `files_after` lists only regular files, so empty directories do not appear in it.
- A table reaches an external command as one JSON object per line, and JSON lines coming back are parsed into rows again. `sort` on that output sorts by the JSON text, which starts with the first column, so `select` the column you want to sort on first.
- `--seed` values travel on the command line, so large inputs hit the OS argument limit. Generate them inside the koa script instead, as the large-input recipe does.
