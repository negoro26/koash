#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
gnu_ls = subprocess.run(["ls", "--version"], capture_output=True, text=True).stdout
if "GNU coreutils" not in gnu_ls:
    sys.exit(f"verify-flags.py compares koa with GNU coreutils, but ls is: {gnu_ls.splitlines()[:1]}")
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
RAW = [
    "echo -n hi", "echo -x hi", "echo hi -n", "echo -- hi", "echo -nx hi",
    "e=; printf '%s|' a $e b \"$e\" c", "e=; $e echo hi", "HOME=; printf '[%s]' ~ x",
]
FINDS = ["find . -name g", "find . -name 'f*'", "find . -type d", "find . -maxdepth 1 -type f", "find d -iname G"]
TREES = [
    "rm -rf d", "rm -r -f d", "rm --recursive --force d", "rm -fr d", "rm -- -x",
    "rm -f missing", "mkdir -p a/b/c", "mkdir --parents a/b", "mkdir -p -- -y",
    "mkdir -p d", "mkdir d", "rm -f",
    "rm -rf ''", "files=; rm -rf $files", "e=; rm -r d $e", "rm -f ''", "rm ''",
    "e=; $e > f", "e=; $e > newfile", "e=; $e < missing", "mkdir -p ''", "mkdir ''",
]
LS_FIXTURE = [
    ("a", 10, 1000, 0), ("b", 30, 3000, 0), ("c", 30, 2000, 0), ("d", 20, 3000, 0),
    ("e", 10, 1000, 500_000_000), ("g", 20, 2000, 0), (".h", 5, 4000, 0), ("B", 30, 3000, 0),
]
LS_ORDERS = [
    "-1", "-1 -t", "-1t", "-t", "-1 -S", "-S", "-1 -r", "-r", "-1 -tr", "-tr", "-1 -rt",
    "-1 -Sr", "-rS", "-St", "-tS", "-t -S", "-S -t", "-t -S -t", "-lt", "-ltr", "-lS", "-lhS",
    "-lhSr", "-lrt", "-lth", "-hlt", "-l1t", "-1ht", "--reverse", "-t --reverse", "-A", "-A -t",
    "-At", "-ASr", "-Atr", "-A1S",
]
LS_FALLBACKS = ["-1 -X", "-1 -tX", "-1 -SX"]


GLOB_FILES = [
    "a.txt", "b.txt", "B.txt", "c.log", "d.log", ".hidden.txt", ".dot.log", "sp ace.txt", "\u00e9.txt",
    "\u00fcn\u00ef.log", "a1", "a2", "a10", "aB", "-dash.txt", "star*.txt", "q?.txt",
    "sub/x.txt", "sub/y.log", "sub/.h", "sub/deep/z.txt", "dir1/f.txt", "dir2/f.txt", "dir.x/q", "dir/q",
    "sp ace/in side.txt", ".hid/in.txt",
]
GLOB_ARGS = [
    "*.txt", "*.log", ".*.txt", ".*", "*", "?.txt", "??.txt", "a[0-9]*", "a[!0-9]*", "a[^0-9]*", "[ab].txt",
    "[A-B].*", "sub/*", "sub/.*", "*/*.txt", "*/", "*/*/", "dir*/", "*/f.txt", "\u00e9*", "[\u00e9]*", "sp*",
    "sp\\ *", "'sp ace'/*", "\"*.txt\"", "'*.txt'", "\\*.txt", "star\\*.txt", "star[*].txt", "*.nomatch",
    "nomatch/*", "*.nomatch *.txt", "a.txt *.nomatch c.log", "[", "[x", "q?.txt", "q\\?.txt", "{a,b}.txt",
    "{a,b,c}.{txt,log}", "{1..5}", "{05..8}", "{a..e}", "{5..1}", "{1..10..3}", "{-2..2}", "x{a,b{c,d}}y",
    "{a}", "{}", "a{b", "{a,b", "{a,{b,c}", "{,a}", "x{,a}", "'{a,b}' \\{a,b}", "{1..a}", "{*,x}.txt",
    "a{1..3}b{x,y}", "{sub,dir1}/*", "{nonexist,dir2}/*", "./*.log", "sub//*", "*/in*", ".h*/*", "*/.h",
    "*.[lt]*", "***.txt", "-*", "[a-c]*.txt", "[!a-c]*.txt", "*/q", "d*/*",
]
GLOB_ASSIGNED = [
    "x='*.txt'; printf '[%s]\\n' $x", "x='*.txt'; printf '[%s]\\n' \"$x\"", "x='a,b'; printf '[%s]\\n' {$x}",
    "x=sub; printf '[%s]\\n' $x/*", "x=; printf '[%s]\\n' $x*.log",
]
GLOB_TREES = [
    "rm *.log", "rm sub/*.txt dir?/*.txt", "mkdir -p out/{x,y}/z", "rm *.nomatch", "mv *.log sub", "cp a1 a2 dir1",
]


def glob_scratch():
    root = tempfile.mkdtemp()
    for name in GLOB_FILES:
        path = os.path.join(root, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, "w").close()
    return root


def scratch():
    root = tempfile.mkdtemp()
    for name in ("f", "-x"):
        with open(os.path.join(root, name), "w") as fh:
            fh.write(TEXT)
    os.makedirs(os.path.join(root, "d/e"))
    open(os.path.join(root, "d/e/g"), "w").close()
    return root


def ls_scratch():
    root = tempfile.mkdtemp()
    for name, size, sec, nsec in LS_FIXTURE:
        path = os.path.join(root, name)
        with open(path, "wb") as fh:
            fh.write(b"x" * size)
        os.utime(path, ns=(sec * 10**9 + nsec, sec * 10**9 + nsec))
    os.mkdir(os.path.join(root, "sub"))
    os.utime(os.path.join(root, "sub"), ns=(500 * 10**9, 500 * 10**9))
    return root


def gnu_names(flags, root):
    long = any(t[0] == "-" and t[1] != "-" and "l" in t for t in flags.split())
    tail = " | sed 1d | awk '{print $NF}'" if long else ""
    return gnu(f"LC_ALL=C ls {flags}{tail}", root).stdout.splitlines()


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
    shutil.rmtree(b, ignore_errors=True)
for cmd in RAW:
    root = scratch()
    g, k = gnu(cmd, root).stdout, koa_run(cmd, root, False).stdout
    if g != k:
        bad.append(f"{cmd!r}: gnu {g!r}, koa {k!r}")
    shutil.rmtree(root)
GLOB_ENV = dict(os.environ, LC_ALL="C.UTF-8")
for args in GLOB_ARGS:
    root = glob_scratch()
    cmd = f"printf '[%s]\\n' {args}"
    want = subprocess.run(["bash", "-c", cmd], cwd=root, capture_output=True, text=True, env=GLOB_ENV).stdout
    got = koa_run(cmd, root, False).stdout
    if got != want:
        bad.append(f"{cmd!r}: bash {want!r}, koa {got!r}")
    shutil.rmtree(root)
for cmd in GLOB_ASSIGNED:
    root = glob_scratch()
    want = subprocess.run(["bash", "-c", cmd], cwd=root, capture_output=True, text=True, env=GLOB_ENV).stdout
    got = koa_run(cmd, root, False).stdout
    if got != want:
        bad.append(f"{cmd!r}: bash {want!r}, koa {got!r}")
    shutil.rmtree(root)
for cmd in GLOB_TREES:
    a, b = glob_scratch(), glob_scratch()
    gr, kr = gnu(cmd, a).returncode, koa_run(cmd, b, False).returncode
    if tree(a) != tree(b) or (gr == 0) != (kr == 0):
        bad.append(f"{cmd!r}: gnu rc={gr} {tree(a)}, koa rc={kr} {tree(b)}")
    shutil.rmtree(a)
    shutil.rmtree(b, ignore_errors=True)
for cmd in FINDS:
    root = scratch()
    want = sorted(p.removeprefix("./") for p in gnu(cmd, root).stdout.splitlines() if p != ".")
    out = koa_run(f"{cmd} | select path | to json", root, False)
    got = sorted(row["path"] for row in json.loads(out.stdout or "[]"))
    if out.returncode != 0 or got != want:
        bad.append(f"{cmd!r}: gnu {want}, koa rc={out.returncode} {got}")
    shutil.rmtree(root)
for flags in LS_ORDERS:
    root = ls_scratch()
    want = gnu_names(flags, root)
    out = koa_run(f"ls {flags} | select name | to json", root, False)
    got = [row["name"] for row in json.loads(out.stdout or "[]")]
    if out.returncode != 0 or got != want:
        bad.append(f"ls {flags}: gnu {want}, koa rc={out.returncode} {got}")
    shutil.rmtree(root)
for flags in LS_FALLBACKS:
    root = ls_scratch()
    want = gnu(f"LC_ALL=C ls {flags}", root).stdout
    out = subprocess.run(
        [koa, "-c", f"ls {flags}"], cwd=root, capture_output=True, text=True,
        env={**os.environ, "LC_ALL": "C"},
    )
    if "is not built in, running" not in out.stderr or out.stdout != want:
        bad.append(f"ls {flags}: must fall back to the real ls, gnu {want!r}, koa {out.stdout!r} {out.stderr!r}")
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

root = scratch()
koa_run("e=; $e > f", root, False)
if os.path.getsize(os.path.join(root, "f")) != 0:
    bad.append("'e=; $e > f': f must be emptied, as bash does")
koa_run("rm -r d; mkdir -p ''; undo", root, False)
if not os.path.isdir(os.path.join(root, "d")):
    bad.append("'rm -r d; mkdir -p ''; undo': undo must restore d")
shutil.rmtree(root)

root = glob_scratch()
before = tree(root)
koa_run("rm *.log; undo", root, False)
if tree(root) != before:
    bad.append("'rm *.log; undo': undo must restore every file the glob removed")
shutil.rmtree(root)

SIGILS = {
    "@sql SELECT '{a,b}' AS v | to json": [{"v": "{a,b}"}],
    "@sql SELECT '*' AS v | to json": [{"v": "*"}],
}
root = glob_scratch()
open(os.path.join(root, "SELECT 'x' AS v"), "w").close()
for cmd, want in SIGILS.items():
    out = koa_run(cmd, root, False)
    if out.returncode != 0 or json.loads(out.stdout or "null") != want:
        bad.append(f"{cmd!r}: sigil code must reach SQLite as typed, want {want}, koa rc={out.returncode} {out.stdout!r} {out.stderr!r}")
shutil.rmtree(root)

root = tempfile.mkdtemp()
open(os.path.join(root, "SELECT 42"), "w").close()
out = koa_run("@sql *", root, False)
if out.returncode == 0 or "42" in out.stdout:
    bad.append(f"'@sql *' next to a file named 'SELECT 42' must reach SQLite as '*', koa rc={out.returncode} {out.stdout!r}")
shutil.rmtree(root)

checked = len(READS) + len(FAILS) + len(USAGE) + len(TREES) + len(RAW) + len(FINDS) + TABLE_FORMS
checked += len(LS_ORDERS) + len(LS_FALLBACKS) + len(GLOB_ARGS) + len(GLOB_ASSIGNED) + len(GLOB_TREES) + len(SIGILS) + 4
print(f"checked {checked} flag forms against GNU")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
