#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$*"; }

step "rustfmt"
cargo fmt --all --check
step "taplo"
taplo fmt --check
step "typos"
typos
step "comment budget"
comments=$(git grep -hE '^\s*//' -- 'crates/*.rs' | wc -l)
budget=$(cat scripts/comment-budget)
if ((comments > budget)); then
  echo "crates/ has $comments comment lines, budget is $budget." >&2
  echo "Keep a comment only for a why the code cannot show. Delete the rest." >&2
  exit 1
fi
echo "$comments of $budget comment lines"
step "clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests"
cargo nextest run --workspace --locked
step "real-binary checks"
cargo build --locked --bin koa
koa="$(realpath "${CARGO_TARGET_DIR:-target}")/debug/koa"
python3 scripts/verify-ls-cells.py "$koa"
python3 scripts/verify-json.py "$koa"
python3 scripts/verify-flags.py "$koa"
step "cargo-deny"
cargo deny --locked check
step "cargo-machete"
cargo machete
