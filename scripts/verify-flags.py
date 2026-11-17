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

checked = len(READS) + len(FAILS) + len(USAGE) + len(TREES) + len(RAW) + len(FINDS)
print(f"checked {checked} flag forms against GNU")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
