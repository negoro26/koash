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

checked = len(READS) + len(FAILS) + len(USAGE) + len(TREES) + len(RAW) + len(FINDS) + len(GLOB_ARGS) + len(GLOB_ASSIGNED) + len(GLOB_TREES) + len(SIGILS) + 4
print(f"checked {checked} flag forms against GNU")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
