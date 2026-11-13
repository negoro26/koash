#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  echo "usage: scripts/profile.sh flamegraph|samply|heaptrack|cachegrind|strace -- <koa args>" >&2
  exit 2
}

[[ $# -ge 2 && $2 == "--" ]] || usage
tool=$1
shift 2

cargo build --profile profiling --bin koa
bin=target/profiling/koa
out=target/profile
mkdir -p "$out"

case $tool in
  flamegraph)
    flamegraph -o "$out/flamegraph.svg" -- "$bin" "$@"
    echo "wrote $out/flamegraph.svg"
    ;;
  samply) samply record -- "$bin" "$@" ;;
  heaptrack)
    heaptrack --record-only -o "$out/heaptrack" "$bin" "$@"
    heaptrack_print "$out/heaptrack.zst" | head -60
    ;;
  cachegrind)
    valgrind --tool=cachegrind --cache-sim=yes --cachegrind-out-file="$out/cachegrind.out" "$bin" "$@"
    cg_annotate "$out/cachegrind.out" | head -40
    ;;
  strace) strace -f -c -o "$out/strace.txt" "$bin" "$@" && cat "$out/strace.txt" ;;
  *) usage ;;
esac
