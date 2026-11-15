#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
depth=${1:-5}
KOA_UNDO_DEPTH=$depth cargo test -q -p shell-builtins --release --locked \
  --test undo_ledger_exhaustive -- --nocapture
