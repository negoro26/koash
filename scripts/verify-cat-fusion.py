#!/usr/bin/env python3
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time

koa = os.path.abspath(sys.argv[1])
bad = []
checked = 0
ENV = {"PATH": os.environ["PATH"], "LC_ALL": "C.UTF-8", "HOME": "/nonexistent"}
BARRIER = "select file line hash text"

BIG_LINES = 200_000
WORDS = ["alpha", "beta", "gamma", "delta", "\u00e9t\u00e9", "\u65e5\u672c\u8a9e", "", "tab\there", "quote\"s", "back\\slash", "x" * 300]


def run(script, cwd, data=b""):
    return subprocess.run(
        [koa, "-c", script], cwd=cwd, env=ENV, capture_output=True, timeout=120, input=data
    )


def fixture():
    root = tempfile.mkdtemp()
    rng = random.Random(5)
    with open(os.path.join(root, "big"), "w", encoding="utf-8") as fh:
        for i in range(BIG_LINES):
            fh.write(f"{i} {rng.choice(WORDS)}\n")
    small = {
        "empty": b"",
        "one": b"only\n",
        "nonl": b"a\nb\nlast-no-newline",
        "crlf": b"a\r\nb\r\nc\r\n",
        "blank": b"\n\n\n",
        "bin": b"ok\n\xff\xfe\x00bad\nafter\n",
        "ten": b"".join(f"line {i}\n".encode() for i in range(1, 11)),
        "sp ace": b"x\ny\nz\n",
        "-r": b"dash r\n",
        "-": b"dash\n",
        "--": b"dashes\n",
    }
    for name, data in small.items():
        with open(os.path.join(root, name), "wb") as fh:
            fh.write(data)
    os.mkdir(os.path.join(root, "sub"))
    os.mkdir(os.path.join(root, "adir"))
    open(os.path.join(root, "locked"), "w").write("secret\n")
    if os.geteuid() != 0:
        os.chmod(os.path.join(root, "locked"), 0)
    return root


def same(label, fused, unfused, cwd, outputs=(), data=b""):
    global checked
    checked += 1
    results = []
    for script in (fused, unfused):
        out = run(script, cwd, data)
        files = {}
        for name in outputs:
            path = os.path.join(cwd, name)
            files[name] = open(path, "rb").read() if os.path.exists(path) else None
            if os.path.exists(path):
                os.remove(path)
        results.append((out, files))
    (got, got_files), (want, want_files) = results
    if got_files != want_files:
        bad.append(f"{label}: {fused!r} wrote different files than the unfused form")
    if (got.stdout, got.stderr, got.returncode) != (want.stdout, want.stderr, want.returncode):
        bad.append(
            f"{label}: {fused!r}: rc {got.returncode} vs {want.returncode}, stdout {len(got.stdout)} vs {len(want.stdout)} bytes, "
            f"stderr {got.stderr[:80]!r} vs {want.stderr[:80]!r}"
        )


root = fixture()
SMALL = ["empty", "one", "nonl", "crlf", "blank", "bin", "ten", "sp ace", "./ten", "sub/../ten", os.path.join(root, "ten")]
COUNTS = ["0", "1", "2", "3", "5", "9", "10", "11", "1000", "99999999999"]
for name in SMALL:
    quoted = "'" + name + "'"
    for verb in ("first", "last"):
        for count in COUNTS:
            same(f"{verb} {count} of {name}", f"cat {quoted} | {verb} {count} | to json",
                 f"cat {quoted} | {BARRIER} | {verb} {count} | to json", root)
        same(f"bare {verb} of {name}", f"cat {quoted} | {verb} | to json", f"cat {quoted} | {BARRIER} | {verb} | to json", root)
        for odd in ("abc", "-1", "1.5", "+2", "2 extra", ""):
            same(f"{verb} {odd!r} of {name}", f"cat {quoted} | {verb} {odd} | to json",
                 f"cat {quoted} | {BARRIER} | {verb} {odd} | to json", root)

for name in ("big",):
    for verb in ("first", "last"):
        for count in ("0", "1", "7", "100000", "199999", "200000", "200001"):
            same(f"{verb} {count} of {name}", f"cat {name} | {verb} {count} | to json",
                 f"cat {name} | {BARRIER} | {verb} {count} | to json", root)

for name in ("nonexistent", "adir", "locked", "sub"):
    for verb in ("first", "last"):
        same(f"{verb} of {name}", f"cat {name} | {verb} 3 | to json", f"cat {name} | {BARRIER} | {verb} 3 | to json", root)

for fused, unfused in [
    ("cat ten | first 3", f"cat ten | {BARRIER} | first 3"),
    ("cat ten | last 3", f"cat ten | {BARRIER} | last 3"),
    ("cat ten | first 5 | last 2 | to json", f"cat ten | {BARRIER} | first 5 | last 2 | to json"),
    ("cat ten | last 5 | first 2 | to json", f"cat ten | {BARRIER} | last 5 | first 2 | to json"),
    ("cat ten | first 8 | where line > 3 | to json", f"cat ten | {BARRIER} | first 8 | where line > 3 | to json"),
    ("cat ten | first 4 | select text | to json", f"cat ten | {BARRIER} | first 4 | select text | to json"),
    ("cat ten | first 2 | cat | to json", f"cat ten | {BARRIER} | first 2 | cat | to json"),
    ("cat ten | first 3 > o1", f"cat ten | {BARRIER} | first 3 > o1"),
    ("cat ten | last 3 > o1", f"cat ten | {BARRIER} | last 3 > o1"),
    ("cat ten | first 3 >> o1", f"cat ten | {BARRIER} | first 3 >> o1"),
    ("cat ten | first 3 2> e1", f"cat ten | {BARRIER} | first 3 2> e1"),
    ("cat ten | first 3 | cat -r", f"cat ten | {BARRIER} | first 3 | cat -r"),
    ("cat -r | first 3", f"cat -r | {BARRIER} | first 3"),
    ("cat - | first 3", f"cat - | {BARRIER} | first 3"),
    ("cat | first 3", f"cat | {BARRIER} | first 3"),
    ("cat ten | first 3 | wc -l", f"cat ten | {BARRIER} | first 3 | wc -l"),
    ("cat ten | first 3; echo $?", f"cat ten | {BARRIER} | first 3; echo $?"),
    ("cat nonexistent | first 3; echo $?", f"cat nonexistent | {BARRIER} | first 3; echo $?"),
    ("cat ten | first 3 && echo ok", f"cat ten | {BARRIER} | first 3 && echo ok"),
    ("true | cat ten | first 3 | to json", f"true | cat ten | {BARRIER} | first 3 | to json"),
    ("cat ten | head -n 3 | to json", f"cat ten | {BARRIER} | head -n 3 | to json"),
    ("cat -r ten | first 3 | to json", f"cat -r ten | {BARRIER} | first 3 | to json"),
    ("cat ten ten | first 3 | to json", f"cat ten ten | {BARRIER} | first 3 | to json"),
    ("cat ten | first 3 | first 2 | to json", f"cat ten | {BARRIER} | first 3 | first 2 | to json"),
]:
    same(fused, fused, unfused, root, outputs=[name for name in ("o1", "e1") if name in fused])

for script in ("cat -r | first 2", "cat - | first 2", "cat -- | first 2", "cat | first 2", "cat | last 2"):
    same(f"stdin {script}", script, script.replace("|", f"| {BARRIER} |", 1), root, data=b"x\ny\nz\n")
for script in ("cat one ten | first 3 | to json", "cat ten one | last 3 | to json", "cat one one | first 3 | to json"):
    same(script, script, script.replace("|", f"| {BARRIER} |", 1), root)

sparse = os.path.join(root, "sparse")
with open(sparse, "wb") as fh:
    fh.write(b"head one\nhead two\nhead three\n")
    fh.truncate(512 * 1024 * 1024)


def timed(script):
    start = time.monotonic()
    out = run(script, root)
    return time.monotonic() - start, out


checked += 1
fused_time, fused_out = timed("cat sparse | first 3 | to json")
unfused_time, unfused_out = timed(f"cat sparse | {BARRIER} | first 3 | to json")
if fused_out.stdout != unfused_out.stdout:
    bad.append("first 3 of a 512 MiB sparse file differs between fused and unfused")
if fused_time * 5 > unfused_time:
    bad.append(f"cat | first 3 on a 512 MiB sparse file took {fused_time:.2f}s, unfused {unfused_time:.2f}s: the file was read in full")

if os.geteuid() != 0:
    os.chmod(os.path.join(root, "locked"), 0o600)
shutil.rmtree(root)
print(f"checked {checked} cat | first and cat | last pipelines against the unfused form")
for line in bad:
    print(line[:400])
sys.exit(1 if bad else 0)
