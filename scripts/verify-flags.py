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
LS_FIXTURE = [
    ("a", 10, 1000, 0), ("b", 30, 3000, 0), ("c", 30, 2000, 0), ("d", 20, 3000, 0),
    ("e", 10, 1000, 500_000_000), ("g", 20, 2000, 0), (".h", 5, 4000, 0),
]
LS_ORDERS = [
    "-1", "-1 -t", "-1t", "-t", "-1 -S", "-S", "-1 -r", "-r", "-1 -tr", "-tr", "-1 -rt",
    "-1 -Sr", "-rS", "-St", "-tS", "-t -S", "-S -t", "-t -S -t", "-lt", "-ltr", "-lS", "-lhS",
    "-lhSr", "-lrt", "-lth", "-hlt", "-l1t", "-1ht", "--reverse", "-t --reverse", "-A", "-A -t",
    "-At", "-ASr", "-Atr", "-A1S",
]
LS_FALLBACKS = ["-1 -X", "-1 -tX", "-1 -SX"]


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

checked = len(READS) + len(FAILS) + len(USAGE) + len(TREES) + len(RAW) + len(FINDS)
checked += len(LS_ORDERS) + len(LS_FALLBACKS)
print(f"checked {checked} flag forms against GNU")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
