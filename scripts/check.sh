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
step "clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests"
cargo nextest run --workspace --locked
step "cargo-deny"
cargo deny --locked check
step "cargo-machete"
cargo machete
