# koash (`koa`)

An agent-native structured command shell written in Rust. `koa` combines POSIX-compatible pipelines with typed record streams, Hashline code anchors, transactional micro-commits, persistent sub-kernels, and headless RPC/ACP modes in a single binary.

## Build

```bash
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

## Key Capabilities

- **Structured Pipelines**: In-process `RecordBatch` streams across `ls`, `stat`, `find`, `ps`, `where`, `select`, `sort-by`, `first`, `last`, `from`, and `to`, automatically bridging to raw byte streams at external binaries.
- **Hashline Anchors & Atomic Edits**: `cat`, `grep`, and `ast` emit deterministic `line:hash` tags verified by `patch`.
- **Code Intelligence**: In-process `prewalk` / `ast` (`tree-sitter`) and warm LSP diagnostics/refactoring via `diag`, `refs`, and `rename`.
- **Transactional Undo**: Reversible file micro-commits via `tx-log`, `undo`, and `rollback`, plus unprivileged CoW session isolation.
- **Persistent Sub-Kernels**: Stateful `@sql` (in-process SQLite), `@py`, and `@js` sessions.
- **Progressive Discovery**: Built-in BM25 command search via `discover "<intent>"`.

## Workspace Layout

| Crate | Role |
| :--- | :--- |
| `shell-syntax` | Zero-copy recursive-descent parser |
| `shell-value` | Hybrid `RecordBatch` / `RawStream` values, Hashline tags, taint tracking |
| `shell-kernel` | Sessions, PTY/process groups, CoW overlay, tx ledger, policy engine |
| `shell-builtins` | Builtin commands, pipelines, `discover`, `code_intel`, LSP, sub-kernels |
| `shell-rpc` | Headless NDJSON `--rpc`, `--acp`, and `--socket` server |
| `shell-surface` | Terminal tables, compact sigil format, and TSP output adapters |
| `shell-tui` | `crossterm` line editor, AST syntax highlighting, completion |
| `shell-cli` | Unified `koa` binary entrypoint |

## License

MIT
