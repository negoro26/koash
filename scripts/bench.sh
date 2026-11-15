#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  echo "usage: scripts/bench.sh <koa> [<koa-to-compare>]" >&2
  echo "  BENCH_RUNS=30 runs per command; BENCH_DATA=target/bench-data holds the generated files" >&2
  exit 2
}
[[ $# -ge 1 && $# -le 2 ]] || usage
for bin in "$@"; do [[ -x $bin ]] || { echo "not executable: $bin" >&2; exit 2; }; done

runs=${BENCH_RUNS:-30}
data=$(realpath -m "${BENCH_DATA:-target/bench-data}")
out=target/bench/$(date +%Y%m%dT%H%M%S)
mkdir -p "$data" "$out"

for n in 20 1000 20000; do
  dir=$data/files-$n
  if [[ ! -d $dir ]]; then
    mkdir -p "$dir"
    (cd "$dir" && seq 1 "$n" | sed 's/^/file_/' | xargs touch)
  fi
done
[[ -f $data/lines.txt ]] || seq 1 200000 | sed 's/^/line number /' > "$data/lines.txt"

echo "machine: $(nproc) cores, load $(cut -d' ' -f1-3 /proc/loadavg)"
for bin in "$@"; do echo "binary:  $bin ($("$bin" --version))"; done

for bin in "$@"; do
  for n in 20 1000 20000; do
    got=$(cd "$data/files-$n" && "$bin" -c 'ls | to json' | grep -c '"name"')
    [[ $got == "$n" ]] || { echo "WRONG: $bin ls in files-$n listed $got rows, expected $n" >&2; exit 1; }
  done
  got=$(cd "$data" && "$bin" -c 'cat lines.txt | last 1 | to json' | grep -c '"line number 200000"')
  [[ $got == 1 ]] || { echo "WRONG: $bin cat|last did not return the last line" >&2; exit 1; }
done
echo "outputs checked: ls row counts and cat|last match the data"

bench() {
  local name=$1 dir=$2
  shift 2
  (cd "$dir" && hyperfine -N -w 3 -r "$runs" --export-json "$OLDPWD/$out/$name.json" "$@" >/dev/null)
}

for n in 20 1000 20000; do
  cmds=()
  for bin in "$@"; do
    cmds+=("$bin -c ls" "$bin -c 'ls | where kind == file | sort-by name | first 1'")
  done
  bench "files-$n" "$data/files-$n" "${cmds[@]}" "/bin/ls -la"
done

cmds=()
for bin in "$@"; do cmds+=("$bin -c 'cat lines.txt | last 1'"); done
bench lines "$data" "${cmds[@]}" "/usr/bin/tail -n 1 lines.txt"

python3 - "$out" "$@" <<'EOF'
import json, pathlib, statistics, sys
out = pathlib.Path(sys.argv[1])
labels = dict(zip(sys.argv[2:], ("A", "B")))
for path, name in labels.items():
    print(f"{name} = {path}")
print(f"\n{'case':<11} {'command':<62} {'median ms':>10} {'min-max ms':>17} {'peak MB':>8}")
for f in sorted(out.glob("*.json")):
    for r in json.loads(f.read_text())["results"]:
        m = r["measurements"]
        t = [x["time_wall_clock"]["value"] * 1000 for x in m]
        rss = statistics.median(x["memory_peak_resident"]["value"] for x in m) / 1e6
        bad = {x.get("exit_code") for x in m} - {0}
        cmd = r["command"]
        for path, name in labels.items():
            cmd = cmd.replace(path, name)
        flag = f"  exit {sorted(bad)}" if bad else ""
        print(f"{f.stem:<11} {cmd[:62]:<62} {statistics.median(t):>10.2f} {min(t):>8.2f}-{max(t):<8.2f} {rss:>8.1f}{flag}")
print(f"\nraw results: {out}")
EOF
