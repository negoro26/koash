#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
TEXT = "".join(f"line {i}\n" for i in range(1, 21))

READS = [
    "head -n5 f", "head -n 5 f", "head --lines=5 f", "head --lines 5 f", "head -5 f",
    "head -n5 -- f", "head -- -x", "head -n 3 -- -x",
    "tail -n5 f", "tail -n 5 f", "tail --lines=5 f", "tail --lines 5 f", "tail -5 f",
    "tail -n2 -- -x",
    "cat f", "cat -- -x", "cat f -- -x",
    "head -n2 -n5 f", "tail -n9 --lines=3 f",
    "grep -i 'LINE 1' f", "grep -iw 'line 1' f", "grep -F 'line 1' f", "grep -v 1 f",
    "grep -Fv 'line 1' f", "grep -wi 'LINE 2' -- f",
]
FAILS = [
    "head -n", "head -n x f", "head --nope f", "tail --lines", "cat --nope f", "rm", "mkdir",
    "rm --force=1 d", "cat -5 f",
]
USAGE = ["rm", "mkdir", "head -n x f", "tail -n x f", "find . -maxdepth x"]
RAW = ["echo -n hi", "echo -x hi", "echo hi -n", "echo -- hi", "echo -nx hi"]
FINDS = ["find . -name g", "find . -name 'f*'", "find . -type d", "find . -maxdepth 1 -type f", "find d -iname G"]
TREES = [
    "rm -rf d", "rm -r -f d", "rm --recursive --force d", "rm -fr d", "rm -- -x",
    "rm -f missing", "mkdir -p a/b/c", "mkdir --parents a/b", "mkdir -p -- -y",
    "mkdir -p d", "mkdir d", "rm -f",
]


def scratch():
    root = tempfile.mkdtemp()
    for name in ("f", "-x"):
        with open(os.path.join(root, name), "w") as fh:
            fh.write(TEXT)
    os.makedirs(os.path.join(root, "d/e"))
    open(os.path.join(root, "d/e/g"), "w").close()
    return root


def tree(root):
    return sorted(
        os.path.relpath(os.path.join(dirpath, name), root)
        for dirpath, dirs, files in os.walk(root)
        for name in dirs + files
        if not os.path.relpath(dirpath, root).startswith(".koa") and name != ".koa"
    )


def gnu(cmd, cwd):
    return subprocess.run(["bash", "-c", cmd], cwd=cwd, capture_output=True, text=True)


bad = []


def koa_run(cmd, cwd, lines, fallback_ok=False):
    script = f"{cmd} | select text | to json" if lines else cmd
    out = subprocess.run([koa, "-c", script], cwd=cwd, capture_output=True, text=True)
    if not fallback_ok and "is not built in, running" in out.stderr:
        bad.append(f"{cmd!r}: koa handed it to the external program, so its own parser was not tested")
    return out


for cmd in READS:
    root = scratch()
    want = gnu(cmd, root).stdout.splitlines()
    out = koa_run(cmd, root, True)
    got = [row["text"] for row in json.loads(out.stdout or "[]")]
    if out.returncode != 0 or got != want:
        bad.append(f"{cmd!r}: koa rc={out.returncode} {got[:3]}..., gnu {want[:3]}...")
    shutil.rmtree(root)
for cmd in FAILS:
    root = scratch()
    g, k = gnu(cmd, root).returncode, koa_run(cmd, root, False, fallback_ok=True).returncode
    if (g == 0) != (k == 0):
        bad.append(f"{cmd!r}: gnu rc={g}, koa rc={k}")
    shutil.rmtree(root)
for cmd in USAGE:
    root = scratch()
    g, k = gnu(cmd, root).returncode, koa_run(cmd, root, False).returncode
    if g == 0 or k != 2:
        bad.append(f"{cmd!r}: gnu rc={g}, koa rc={k}, a usage error must exit 2")
    shutil.rmtree(root)
for cmd in TREES:
    a, b = scratch(), scratch()
    gr, kr = gnu(cmd, a).returncode, koa_run(cmd, b, False).returncode
    if tree(a) != tree(b) or (gr == 0) != (kr == 0):
        bad.append(f"{cmd!r}: gnu rc={gr} {tree(a)}, koa rc={kr} {tree(b)}")
    shutil.rmtree(a)
    shutil.rmtree(b)
for cmd in RAW:
    root = scratch()
    g, k = gnu(cmd, root).stdout, koa_run(cmd, root, False).stdout
    if g != k:
        bad.append(f"{cmd!r}: gnu {g!r}, koa {k!r}")
    shutil.rmtree(root)
for cmd in FINDS:
    root = scratch()
    want = sorted(p.removeprefix("./") for p in gnu(cmd, root).stdout.splitlines() if p != ".")
    out = koa_run(f"{cmd} | select path | to json", root, False)
    got = sorted(row["path"] for row in json.loads(out.stdout or "[]"))
    if out.returncode != 0 or got != want:
        bad.append(f"{cmd!r}: gnu {want}, koa rc={out.returncode} {got}")
    shutil.rmtree(root)

TABLE_FILES = ["Bar.RS", "baz.txt", "foo.rs", "FOO.md", "my file.rs", "foobar.txt", "sub/foo.c", "sub/Zed.rs"]
TABLE_TEXT = "alpha\n\nBeta\nfoo bar\nfoobar\nFOO\nrs\n"
LS_GREPS = [
    "foo", "-i foo", "-v foo", "-iv foo", "-w foo", "-wi foo", "-F .rs", "-Fi .RS", "-Fv .rs",
    "rs", "'^b'", "-i '^b'", "-E 'rs$|md$'", "-- -x",
    ("-c rs", "count"), ("-vc rs", "count"), ("-ic RS", "count"),
    ("-n rs", "number"), ("-nv rs", "number"), ("-n -i foo", "number"),
]
FIND_GREPS = [
    "foo", "-i foo", "-v foo", "-iv foo", "-w foo", "-wi foo", "-F zed", "-Fi ZED",
    ("-c foo", "count"), ("-ic FOO", "count"),
]
CAT_GREPS = [
    "foo", "-i foo", "-v foo", "-iv FOO", "-w foo", "-wi FOO", "-F 'foo bar'", "-v ''", "''",
    "'^$'", "-v '^$'", ("-c foo", "count"), ("-vc foo", "count"), ("-c ''", "count"),
]
ENV_GREPS = [
    "PATH", "-i path", "-F PATH", "-w PATHX", "-i pathy", "-w zzz", ("-c PATH", "count"), ("-ic path", "count"),
]
ENV_VARS = ["A=1", "PATHX=/x", "pathy=2", "B=zzzpathy", "C=zzz pathy", "PATH=/usr/bin:/bin"]
PS_FLAGS = ["", "-i ", "-w ", "-iw "]


def table_scratch():
    root = tempfile.mkdtemp()
    os.makedirs(os.path.join(root, "sub"))
    for name in TABLE_FILES + ["-x"]:
        open(os.path.join(root, name), "w").close()
    with open(os.path.join(root, "t"), "w") as fh:
        fh.write(TABLE_TEXT)
    return root


def table_case(label, root, source, case, column, sort=False, strip_dot=False):
    args, mode = case if isinstance(case, tuple) else (case, "")
    tail = {"count": "", "number": f" | select line {column}", "": f" | select {column}"}[mode]
    out = subprocess.run(
        [koa, "-c", f"{source} | grep {args}{tail} | to json"], cwd=root, capture_output=True, text=True,
    )
    rows = json.loads(out.stdout or "[]")
    want = gnu(f"LC_ALL=C {source} | grep {args}", root).stdout.splitlines()
    if mode == "count":
        have = [str(rows[0]["count"] if rows else 0)]
    elif mode == "number":
        have = [f"{row['line']}:{row[column]}" for row in rows]
    else:
        have = [row[column] for row in rows]
        if strip_dot:
            want = [p.removeprefix("./") for p in want if p != "."]
    if sort:
        have, want = sorted(have), sorted(want)
    if out.returncode not in (0, 1) or have != want:
        bad.append(f"{label} | grep {args}: koa rc={out.returncode} {have[:4]}..., gnu {want[:4]}...")


root = table_scratch()
for case in LS_GREPS:
    table_case("ls", root, "ls", case, "name")
for case in FIND_GREPS:
    table_case("find .", root, "find .", case, "path", sort=True, strip_dot=True)
for case in CAT_GREPS:
    table_case("cat t", root, "cat t", case, "text")
shutil.rmtree(root)

marker = f"kpsmarker{os.getpid()}"
proc = subprocess.Popen(["bash", "-c", f"exec -a {marker} sleep 60"])
try:
    for flags in PS_FLAGS:
        arg = f"[k]{marker[1:]}"
        arg = arg.upper() if "i" in flags else arg
        listing = gnu(f"ps -eo pid,args | grep {flags}{arg}", "/").stdout.split()
        want = [w for w in listing if w.isdigit() and int(w) == proc.pid]
        out = subprocess.run(
            [koa, "-c", f"ps | grep {flags}{arg} | select pid | to json"], capture_output=True, text=True,
        )
        have = [str(row["pid"]) for row in json.loads(out.stdout or "[]") if row["pid"] == proc.pid]
        if have != want or have != [str(proc.pid)]:
            bad.append(f"ps | grep {flags}{arg}: koa {have}, gnu {want}, started {proc.pid}")
finally:
    proc.kill()
    proc.wait()

for case in ENV_GREPS:
    args, mode = case if isinstance(case, tuple) else (case, "")
    listing = subprocess.run(
        ["env", "-i", *ENV_VARS, "bash", "-c", f"env | grep {args}"], capture_output=True, text=True,
    ).stdout.splitlines()
    tail = "" if mode == "count" else " | select key"
    out = subprocess.run(
        ["env", "-i", *ENV_VARS, koa, "-c", f"env | grep {args}{tail} | to json"], capture_output=True, text=True,
    )
    rows = json.loads(out.stdout or "[]")
    if mode == "count":
        have, want = [str(rows[0]["count"] if rows else 0)], listing
    else:
        have, want = sorted(row["key"] for row in rows), sorted(line.split("=", 1)[0] for line in listing)
    if out.returncode not in (0, 1) or have != want:
        bad.append(f"env | grep {args}: koa rc={out.returncode} {have}, gnu {want}")

TABLE_FORMS = len(LS_GREPS) + len(FIND_GREPS) + len(CAT_GREPS) + len(ENV_GREPS) + len(PS_FLAGS)
checked = len(READS) + len(FAILS) + len(USAGE) + len(TREES) + len(RAW) + len(FINDS) + TABLE_FORMS
print(f"checked {checked} flag forms against GNU")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
