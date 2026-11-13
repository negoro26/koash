# koash (`koa`)

An agent-native structured command shell written in Rust. `koa` runs POSIX pipelines alongside typed record tables, tags lines with Hashline anchors that `patch` verifies, records file changes so `undo` can revert them, keeps `@py`/`@js`/`@sql` sub-kernels alive between commands, and serves headless RPC and ACP modes from one binary.

## Install

Prebuilt Linux binaries are attached on [GitHub Releases](https://github.com/negoro26/koash/releases):

```bash
# Cargo
cargo install --git https://github.com/negoro26/koash

# Or build from source
cargo build --release --bin koa
```

The release binary is emitted at `target/release/koa`.

## Usage

```bash
# Interactive TUI shell
koa

# One-shot execution
koa -c "find crates | where ext == rs | sort-by path"
koa -c "cat src/main.rs | grep SurfaceMode"
koa -c 'discover "search code"'
koa -c "@sql SELECT 1 + 1 AS x"

# Agent / programmatic modes
koa --rpc
koa --acp
koa --socket /tmp/koa.sock
koa --tsp -c "ls"
```

## What works today

- **Structured pipelines.** `ls`, `stat`, `find`, `ps`, `cat`, `grep` and `env` produce tables that `where`, `select`, `sort-by`, `first`, `last`, `from` and `to` filter in-process. At an external command the table becomes NDJSON or text, and external output comes back as lines.
- **Hashline anchors.** `cat`, `grep` and `ast` print a `line:hash` tag per line. `patch file N:hash 'text'` refuses the edit when line `N` has changed since you read it.
- **Undo.** Builtin file changes (`>`/`>>` redirects, `rm`, `mkdir`, `cp`, `mv`, `patch`, `rename`) are recorded per command. `undo` reverts the last one and `rollback <tx>` reverts back to a point. After an external command runs, `undo` refuses with `E_TX_OPAQUE` instead of guessing, because koa cannot see what the external program wrote.
- **Sub-kernels.** `@sql` runs in an in-process SQLite database. `@py` and `@js` keep a `python3` or `node` worker alive per session, so variables survive between commands.
- **Discovery.** `discover "<intent>"` ranks builtins by BM25 over their descriptions.
- **Code tools.** `prewalk` and `ast` parse source with tree-sitter. `diag` runs `cargo check`. `refs` and `rename` are text search and replace over the workspace, not LSP-backed.
- **Agent modes.** `koa --rpc` (newline-delimited JSON over stdio), `koa --socket PATH` (the same protocol over a Unix socket) and `koa --acp` (Agent Client Protocol). Each RPC session has its own state; `exec` supports `timeout_ms`, which kills the whole process group, and a `policy` object.

## Known limits

- Builtins pass whole tables between stages; they do not stream yet. A builtin in a background pipeline (`ls &`) fails with `E_BG_BUILTIN`.
- The RPC `policy` matches commands by name. `/usr/bin/touch` or `sh -c` get past it, so it is not a sandbox.
- `session.fork` with isolation creates a session directory under `.koa/sessions/`, but writes are not redirected into it yet, so forked sessions are not isolated.
- Lines that use `$(...)`, `${x:-y}`, backticks, subshells, here-docs or `if`/`for`/`while` run through `sh -c` as a whole. Structured output, undo tracking and RPC policy checks do not apply inside them.
- Linux only. macOS is untested.

## Workspace layout

| Crate | Role |
| :--- | :--- |
| `shell-syntax` | Lexer and recursive-descent parser |
| `shell-value` | `Value` (`Empty`, `Bytes`, `Batch`), `RecordBatch`, Hashline tags, taint and secret redaction |
| `shell-kernel` | `Session`, process spawning and job control, kill switch, undo ledger, CoW session directories |
| `shell-builtins` | Builtin commands, the pipeline executor, `discover`, code tools, sub-kernels |
| `shell-rpc` | `--rpc`, `--socket` and `--acp` servers, RPC policy checks |
| `shell-surface` | Terminal tables, compact sigil format, TSP output |
| `shell-tui` | `crossterm` line editor, syntax highlighting, completion |
| `shell-cli` | The `koa` binary |

## Development

`scripts/check.sh` is the gate CI runs. It runs rustfmt, taplo, typos, clippy with `-D warnings`, cargo-nextest, cargo-deny and cargo-machete. Clippy denies `unwrap`, `expect` and `panic` outside tests.

```bash
scripts/check.sh                       # full gate
bacon clippy                           # watch mode
cargo nextest run                      # tests; .config/nextest.toml kills a test after 60 s
cargo llvm-cov nextest --html          # coverage report in target/llvm-cov/html
```

With a distro Rust instead of rustup, point coverage at the system LLVM: `LLVM_COV=/usr/bin/llvm-cov LLVM_PROFDATA=/usr/bin/llvm-profdata`.

Profiling builds use the `profiling` profile, which is release with symbols. `scripts/profile.sh` builds it and runs one tool against `koa`. `flamegraph` and `samply` sample through `perf_event_open`, which needs `kernel.perf_event_paranoid` set to 1 or lower.

```bash
scripts/profile.sh flamegraph -- -c 'find crates | where ext == rs | sort-by path'
scripts/profile.sh samply     -- -c '...'   # opens the Firefox Profiler
scripts/profile.sh heaptrack  -- -c '...'   # allocations
scripts/profile.sh cachegrind -- -c '...'   # cache misses
scripts/profile.sh strace     -- -c '...'   # syscall counts
hyperfine -N "target/release/koa -c 'ls | first 1'" "bash -c 'ls | head -1'"
```

To debug, use `lldb-dap` (any DAP client) or `rust-lldb` / `rust-gdb`. Load `$(rustc --print sysroot)/lib/rustlib/etc/lldb_lookup.py` in lldb-dap's `initCommands` so `Vec`, `String` and `PathBuf` print readably. Dependencies build with line tables only, and workspace crates keep full debug info.

## License

MIT
