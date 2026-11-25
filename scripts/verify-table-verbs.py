#!/usr/bin/env python3
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
from collections import Counter

koa = os.path.abspath(sys.argv[1])
gnu = subprocess.run(["ls", "--version"], capture_output=True, text=True).stdout
if "GNU coreutils" not in gnu:
    sys.exit(f"verify-table-verbs.py compares koa with GNU coreutils, but ls is: {gnu.splitlines()[:1]}")

bad = []
checked = 0


def expect(ok, message):
    global checked
    checked += 1
    if not ok:
        bad.append(message)


def run_koa(script, cwd):
    return subprocess.run([koa, "-c", script], cwd=cwd, capture_output=True, text=True, timeout=30)


def rows(script, cwd):
    out = run_koa(script, cwd)
    if out.returncode != 0:
        bad.append(f"koa -c {script!r} exited {out.returncode}: {out.stderr.strip()}")
        return []
    return json.loads(out.stdout)


def gnu_lines(script, cwd):
    out = subprocess.run(["bash", "-c", script], cwd=cwd, capture_output=True, text=True, timeout=30)
    return out.stdout.splitlines()


def texts(script, cwd):
    return [r["text"] for r in rows(script + " | select text | to json", cwd)]


root = tempfile.mkdtemp()
rng = random.Random(11)
WORDS = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta"]
LINE_COUNTS = {"empty": 0, "one": 1, "two": 2, "odd": 7, "even": 200}
for name, count in LINE_COUNTS.items():
    with open(os.path.join(root, name), "w") as fh:
        fh.writelines(f"{rng.choice(WORDS)}\n" for _ in range(count))
for name in LINE_COUNTS:
    with open(os.path.join(root, name)) as src, open(os.path.join(root, f"sorted-{name}"), "w") as dst:
        dst.writelines(sorted(src.readlines()))
for name in ("visible-a", "visible-b", ".dot"):
    open(os.path.join(root, name), "w").close()

for name, count in LINE_COUNTS.items():
    n = rows(f"cat --raw {name} | count | to json", root)
    expect(n and n[0]["count"] == count, f"count of {name}: koa {n}, want {count}")
    n = rows(f"cat --raw {name} | length | to json", root)
    expect(n and n[0]["count"] == count, f"length alias of {name}: koa {n}, want {count}")
    want = gnu_lines(f"wc -l < {name}", root)
    expect(want == [str(count)], f"wc -l {name}: {want}, fixture says {count}")

for script in ["seq 1 1000", "seq 1 1", "printf ''", "printf 'a\\nb\\n'", "ls"]:
    want = int(gnu_lines(f"{script} | wc -l", root)[0])
    got = rows(f"{script} | count | to json", root)
    expect(got and got[0]["count"] == want, f"{script} | count: koa {got}, wc -l says {want}")

want = int(gnu_lines("printf 'a\\nb' | grep -c ''", root)[0])
got = rows("printf 'a\\nb' | count | to json", root)
expect(got and got[0]["count"] == want, f"unterminated last line: koa {got}, grep -c '' says {want}")

for name in LINE_COUNTS:
    want = gnu_lines(f"tac {name}", root)
    got = texts(f"cat {name} | reverse", root)
    expect(got == want, f"reverse {name}: koa {got[:3]}..., tac {want[:3]}...")

for name, count in LINE_COUNTS.items():
    for n in [0, 1, 2, 5, count - 1, count, count + 5]:
        if n < 0:
            continue
        want = gnu_lines(f"tail -n +{n + 1} {name}", root)
        got = texts(f"cat {name} | skip {n}", root)
        expect(got == want, f"skip {n} of {name}: koa {got[:3]}..., tail -n +{n + 1} {want[:3]}...")
    want = gnu_lines(f"tail -n +2 {name}", root)
    got = texts(f"cat {name} | skip", root)
    expect(got == want, f"bare skip of {name}: koa {got[:3]}..., tail -n +2 {want[:3]}...")

for name in LINE_COUNTS:
    for source in (name, f"sorted-{name}"):
        with open(os.path.join(root, source)) as src, open(os.path.join(root, f"table-{source}.json"), "w") as dst:
            json.dump([{"text": line} for line in src.read().split()], dst)
    want = [
        (line.split(None, 1)[1], int(line.split(None, 1)[0]))
        for line in gnu_lines(f"uniq -c sorted-{name}", root)
    ]
    got = [(r["text"], r["count"]) for r in rows(f"cat --raw table-sorted-{name}.json | from json | uniq -c text | to json", root)]
    expect(got == want, f"uniq -c text on sorted-{name}: koa {got}, uniq -c {want}")
    unsorted = Counter(open(os.path.join(root, name)).read().split())
    got = {r["text"]: r["count"] for r in rows(f"cat --raw table-{name}.json | from json | uniq -c text | to json", root)}
    expect(got == dict(unsorted), f"uniq -c text on {name}: koa {got}, counted {dict(unsorted)}")
    want_unique = gnu_lines(f"sort -u {name} | wc -l", root)[0]
    got = rows(f"cat --raw table-{name}.json | from json | uniq text | count | to json", root)
    expect(got and str(got[0]["count"]) == want_unique, f"uniq text on {name}: koa {got}, sort -u says {want_unique}")

def tee_result(tool, name, flag, existing):
    workdir = tempfile.mkdtemp()
    shutil.copy(os.path.join(root, name), workdir)
    if existing:
        with open(os.path.join(workdir, "out"), "w") as fh:
            fh.write("previous\n")
    if tool == "koa":
        out = run_koa(f"cat {name} | tee {flag}out | count | to json", workdir)
        counted = json.loads(out.stdout)[0]["count"]
    else:
        out = subprocess.run(
            ["bash", "-c", f"cat {name} | tee {flag}out | wc -l"], cwd=workdir, capture_output=True, text=True
        )
        counted = int(out.stdout)
    with open(os.path.join(workdir, "out")) as fh:
        content = fh.read()
    shutil.rmtree(workdir)
    return counted, content


for flag in ("", "-a "):
    for name in ("odd", "even"):
        for existing in (False, True):
            got = tee_result("koa", name, flag, existing)
            want = tee_result("bash", name, flag, existing)
            expect(got == want, f"tee {flag}{name} existing={existing}: koa {got[0]} lines {len(got[1])} bytes, bash {want[0]} lines {len(want[1])} bytes")

workdir = tempfile.mkdtemp()
shutil.copy(os.path.join(root, "odd"), workdir)
with open(os.path.join(workdir, "keep"), "w") as fh:
    fh.write("original\n")
run_koa("cat odd | tee keep fresh; undo", workdir)
expect(open(os.path.join(workdir, "keep")).read() == "original\n", "undo did not restore an overwritten tee target")
expect(not os.path.exists(os.path.join(workdir, "fresh")), "undo left a file that tee created")
run_koa("cat odd | tee -a keep", workdir)
appended = open(os.path.join(workdir, "keep")).read()
expect(appended.startswith("original\n") and len(appended) > len("original\n"), "tee -a did not append")
with open(os.path.join(workdir, "keep"), "w") as fh:
    fh.write("original\n")
run_koa("cat odd | tee -a keep; undo", workdir)
expect(open(os.path.join(workdir, "keep")).read() == "original\n", "undo did not restore a tee -a target")
shutil.rmtree(workdir)

with open(os.path.join(root, "aba"), "w") as fh:
    fh.write("a\nb\na\n")
with open(os.path.join(root, "w"), "w") as fh:
    fh.write("b\na\nb\nc\nb\na\n")
with open(os.path.join(root, "crlf"), "w") as fh:
    fh.write("a\r\na\r\nb\r\n")
with open(os.path.join(root, "raw"), "wb") as fh:
    fh.write(b"\xff\nb\n\xff\n\xff\nlast")


def snapshot(workdir):
    seen = {}
    for name in sorted(os.listdir(workdir)):
        if name.startswith("."):
            continue
        with open(os.path.join(workdir, name), "rb") as fh:
            seen[name] = fh.read()
    return seen


def run_both(script):
    results = []
    for argv in ([koa, "-c", script], ["bash", "-c", script]):
        workdir = tempfile.mkdtemp()
        shutil.copytree(root, workdir, dirs_exist_ok=True)
        out = subprocess.run(argv, cwd=workdir, capture_output=True, timeout=30, stdin=subprocess.DEVNULL)
        results.append((out.stdout, out.stderr, out.returncode, snapshot(workdir)))
        shutil.rmtree(workdir)
    return results


TEXT_CASES = [
    "printf 'a\\nb\\na\\n' | uniq | wc -l",
    "printf 'a\\nb\\na\\n' | uniq",
    "sort w | uniq -c | sort -rn",
    "sort w | uniq -c > counted; od -c counted",
    "sort w | uniq",
    "uniq aba",
    "uniq -c aba",
    "uniq aba out; od -c out",
    "uniq -c < aba",
    "uniq nonexistent",
    "uniq aba out extra",
    "cat aba | uniq",
    "cat aba | uniq -c",
    "cat aba | uniq | wc -l",
    "grep a w | uniq -c",
    "head -n 4 w | uniq -c",
    "tail -n 4 w | uniq",
    "uniq -c < crlf | od -c",
    "uniq -c crlf | od -c",
    "uniq -c < raw | od -c",
    "printf '' | uniq -c | od -c",
    "echo hi | tee /nonexistent/x",
    "echo hi | tee /nonexistent/x kept",
    "echo hi | tee kept; od -c kept",
    "printf 'x\\ny' | tee kept | od -c; od -c kept",
    "cat aba | tee kept | wc -l; od -c kept",
    "cat aba | tee kept",
    "tee kept < crlf | od -c; od -c kept",
    "cat aba | tee -a w | wc -l; od -c w",
    "head -n 2 w | tee kept; od -c kept",
    "grep b w | tee kept",
    "tee kept < aba; od -c kept",
    "tee kept",
    "tee",
    "echo hi | tee",
    "echo hi | tee kept /nonexistent/x kept2; echo $?; od -c kept kept2",
]
for script in TEXT_CASES:
    got, want = run_both(script)
    expect(got == want, f"text through uniq or tee differs from bash: {script!r}\n    koa  {got}\n    bash {want}")

workdir = tempfile.mkdtemp()
shutil.copy(os.path.join(root, "aba"), workdir)
with open(os.path.join(workdir, "keep"), "w") as fh:
    fh.write("original\n")
run_koa("uniq aba keep; undo", workdir)
expect(open(os.path.join(workdir, "keep")).read() == "original\n", "undo did not restore a uniq output file")
shutil.rmtree(workdir)

DATA = [
    {"lang": "rust", "lines": 100, "ratio": 1.5, "ok": True},
    {"lang": "go", "lines": 50, "ratio": 2.0, "ok": False},
    {"lang": "rust", "lines": 200, "ratio": 0.25, "ok": True},
    {"lang": "py", "lines": 25, "ratio": 4.0, "ok": True},
    {"lang": "go", "lines": 75, "ratio": 3.5, "ok": False},
    {"lang": "rust", "lines": 300, "ratio": 0.5, "ok": True},
]
with open(os.path.join(root, "data.json"), "w") as fh:
    json.dump(DATA, fh)
FROM = "cat --raw data.json | from json"

by_lang = Counter(r["lang"] for r in DATA)
got = rows(f"{FROM} | group-by lang | to json", root)
want = sorted(by_lang.items(), key=lambda kv: (-kv[1], kv[0]))
expect([(r["group"], r["count"]) for r in got] == want, f"group-by lang: koa {got}, counted {want}")
got = rows(f"{FROM} | group-by --asc lang | to json", root)
want = sorted(by_lang.items(), key=lambda kv: (kv[1], kv[0]))
expect([(r["group"], r["count"]) for r in got] == want, f"group-by --asc lang: koa {got}, counted {want}")
expect(rows(f"{FROM} | group-by missing | to json", root) == [], "group-by on a missing column is not empty")

for column in ("lines", "ratio"):
    values = [r[column] for r in DATA]
    got = rows(f"{FROM} | stats {column} | to json", root)
    want = {
        "column": column,
        "count": len(values),
        "sum": sum(values),
        "min": min(values),
        "max": max(values),
        "avg": round(sum(values) / len(values), 3),
    }
    expect(len(got) == 1 and all(got[0][k] == v for k, v in want.items()), f"stats {column}: koa {got}, computed {want}")
got = rows(f"{FROM} | measure lines | to json", root)
expect(got and got[0]["count"] == len(DATA), f"measure alias: {got}")

got = {r["column"]: r for r in rows(f"{FROM} | schema | to json", root)}
want = {"lang": "str", "lines": "uint", "ratio": "float", "ok": "bool"}
expect({k: v["kind"] for k, v in got.items()} == want, f"schema kinds: koa {got}, want {want}")
expect(all(v["rows"] == len(DATA) and v["non_null"] == len(DATA) for v in got.values()), f"schema counts: {got}")
got = rows(f"{FROM} | describe | to json", root)
expect(len(got) == 4, f"describe alias: {got}")

got = rows(f"{FROM} | reject lines ratio | to json", root)
expect(got == [{"lang": r["lang"], "ok": r["ok"]} for r in DATA], f"reject lines ratio: {got}")
expect(rows(f"{FROM} | reject nope | to json", root) == DATA, "reject on a missing column changed the table")

BIG = [{"n": 9007199254740993}, {"n": 1}, {"n": -9007199254740995}, {"n": 18446744073709551615}]
with open(os.path.join(root, "big.json"), "w") as fh:
    json.dump(BIG, fh)
got = rows("cat --raw big.json | from json | stats n | to json", root)
values = [r["n"] for r in BIG]
want = {"count": len(values), "sum": sum(values), "min": min(values), "max": max(values)}
expect(len(got) == 1 and all(got[0][k] == v for k, v in want.items()), f"stats on integers beyond 2^53: koa {got}, exact {want}")

got = rows(f"{FROM} | group-by lang | uniq -c | to json", root)
expect(
    [(r["group"], r["count"], r.get("uniq_count")) for r in got] == [(g, n, 1) for g, n in sorted(by_lang.items(), key=lambda kv: (-kv[1], kv[0]))],
    f"uniq -c on a table with a count column: {got}",
)

ls_names = sorted(gnu_lines("ls", root))
got = rows("ls | uniq name | select name | to json", root)
expect(sorted(r["name"] for r in got) == ls_names, f"ls | uniq name: koa {got}, ls says {ls_names}")
got = rows(f"{FROM} | tee kept | count | to json", root)
expect(got and got[0]["count"] == len(DATA), f"tee on a table does not pass it through: {got}")

shutil.rmtree(root)
print(f"checked {checked} table verb results against GNU tools and computed expectations")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
