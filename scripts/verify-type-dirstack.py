#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
bad = []
checked = 0
ENV = {"PATH": os.environ["PATH"], "LC_ALL": "C.UTF-8", "HOME": "/nonexistent"}

STACK_SCRIPTS = [
    "pushd a; pushd b; pushd c",
    "pushd a; popd",
    "pushd a; pushd b; popd; popd",
    "pushd a; pushd ../b; popd",
    "pushd a; pushd",
    "pushd a; pushd; pushd",
    "pushd a; pushd b; pushd; popd; popd",
    "pushd",
    "popd",
    "pushd a; popd; popd",
    "pushd nonexistent",
    "pushd a; pushd nonexistent; dirs",
    "pushd file",
    "pushd a/sub",
    "pushd a; pushd sub; pushd ..; pushd ..",
    "pushd .",
    "pushd ..",
    "pushd a/..",
    "pushd a/../b",
    "pushd ./a/./sub/../sub",
    "pushd link",
    "pushd link/sub",
    "pushd link; pushd ..",
    "pushd a; pushd link",
    "pushd /",
    "pushd /; pushd /tmp; popd; popd",
    "pushd a b",
    "popd a",
    "cd a; pushd ../b; popd",
    "cd a; pushd ../b; pushd; popd",
    "pushd a; cd sub; popd",
    "pushd a; cd sub; popd; dirs",
    "pushd a; cd /; popd",
    "dirs",
    "pushd a; cd -",
    "pushd a; pushd b; cd -",
    "pushd a; popd; cd -",
    "cd a; cd -; pushd b",
    "cd a; dirs",
    "pushd a; pushd b; dirs; popd; dirs",
    "pushd a && pushd b && popd && popd && dirs",
    "pushd nonexistent || dirs",
    "popd || dirs",
    "pushd a; pushd b; pushd c; popd; popd; popd; popd",
    "pushd 'dir with space'",
    "pushd 'dir with space'; pushd a; popd; popd",
    "pushd a; pushd ../b; rm -r ../a; popd; dirs",
    "pushd a; pushd ../b; rm -r ../a; popd; popd",
    "pushd ''",
    "pushd a; pushd ''",
    "pushd noperm",
    "pushd a; pushd ../noperm; dirs",
    "cd noperm",
    "cd a; pushd -",
    "cd a; pushd -; pushd -; popd",
    "cd a; pushd ../b; pushd -; popd",
    "pushd -",
    "CDPATH=$PWD/a; cd b; pushd sub",
    "CDPATH=$PWD/a; cd b; pushd sub; popd",
    "CDPATH=$PWD/a; pushd c; popd",
]
KOA_ONLY_STACKS = {
    "pushd a; pushd ../b; rm -r ../a; pushd; dirs": ("$ROOT/b|$ROOT/a $ROOT/b", ["$ROOT/b", "$ROOT/a", "$ROOT"]),
}
REFUSED = ["pushd +0", "pushd +1", "pushd -1", "popd +1", "popd -1", "type -a echo", "type -p sh", "type -P sh"]
NAMES = [
    "cd", "pwd", "echo", "type", "export", "unset", "alias", "unalias", "history", "jobs", "fg", "bg", "wait",
    "exit", "kill", "pushd", "popd", "dirs",
    "sh", "bash", "sort", "wc", "python3", "sed", "awk", "xargs",
    "nonexistent-command-xyz", "a.b", "", "ll", "gs", "e",
    "./x", "./file", "./a", "./nonexistent", "a/../x", "a/x", "/usr/bin/sh", "/usr/bin",
]
TYPE_T_NAMES = [
    ["cd"], ["pushd"], ["dirs"], ["sh"], ["python3"], ["nonexistent-command-xyz"], ["ll"], ["./x"], ["./file"], ["./a"],
    ["cd", "sh", "nonexistent-command-xyz", "ll", "./x"],
]
ALIASES = {"ll": "ls -la", "gs": "git status", "e": "echo hi"}


def run(argv, script, cwd):
    try:
        return subprocess.run(argv + [script], cwd=cwd, env={**ENV, "PWD": cwd}, capture_output=True, timeout=20)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, -9, b"", b"timeout")


def fixture():
    root = os.path.realpath(tempfile.mkdtemp())
    for path in ("a/sub", "b", "c", "dir with space"):
        os.makedirs(os.path.join(root, path))
    open(os.path.join(root, "file"), "w").close()
    open(os.path.join(root, "x"), "w").close()
    os.chmod(os.path.join(root, "x"), 0o755)
    os.mkdir(os.path.join(root, "noperm"))
    os.chmod(os.path.join(root, "noperm"), 0)
    os.symlink("a", os.path.join(root, "link"))
    return root


def cleanup(root):
    os.chmod(os.path.join(root, "noperm"), 0o700)
    shutil.rmtree(root)


def koa_result(script, root):
    out = run([koa, "-c"], f"{script}\necho @@; echo \"$PWD|$OLDPWD\"; pwd | to json; dirs | to json", root)
    text = out.stdout.partition(b"@@\n")[2].decode()
    env_line, _, text = text.partition("\n")
    try:
        pwd_rows, dir_rows = (json.loads(blob) for blob in text.replace("]\n[", "]\x00[").split("\x00"))
        cwd = env_line.replace(root, "$ROOT") + " " + pwd_rows[0]["path"].replace(root, "$ROOT")
        rows = [row["path"].replace(root, "$ROOT") for row in dir_rows]
    except (ValueError, IndexError, KeyError):
        cwd, rows = f"unparsable {text[:80]!r}", []
    return (run([koa, "-c"], script, root).returncode, cwd, rows)


def stack(out, root):
    return [line.replace(root, "$ROOT") for line in out.decode().splitlines()]


for script in STACK_SCRIPTS:
    checked += 1
    results = []
    for tool in ("bash", "koa"):
        root = fixture()
        if tool == "bash":
            out = run(["bash", "-c"], f"{script}\necho @@; echo \"$PWD|$OLDPWD\"; pwd; dirs -l -p", root)
            lines = stack(out.stdout.partition(b"@@\n")[2], root)
            rc = run(["bash", "-c"], script, root).returncode
            result = (rc, lines[0] + " " + lines[1], lines[2:])
        else:
            result = koa_result(script, root)
        cleanup(root)
        results.append(result)
    want, got = results
    if (want[0] == 0) != (got[0] == 0) or want[1:] != got[1:]:
        bad.append(f"{script!r}: bash rc={want[0]} cwd={want[1]} stack={want[2]}; koa rc={got[0]} cwd={got[1]} stack={got[2]}")

for script, (want_cwd, want_rows) in KOA_ONLY_STACKS.items():
    checked += 1
    root = fixture()
    rc, cwd, rows = koa_result(script, root)
    cleanup(root)
    if rc == 0 or cwd != want_cwd or rows != want_rows:
        bad.append(f"{script!r}: want a failure leaving cwd={want_cwd} stack={want_rows}; koa rc={rc} cwd={cwd} stack={rows}")

for script in REFUSED:
    checked += 1
    root = fixture()
    rc = run([koa, "-c"], script, root).returncode
    cleanup(root)
    if rc != 2:
        bad.append(f"{script!r}: koa should refuse with a usage error (rc 2), got rc={rc}")

for names in TYPE_T_NAMES:
    checked += 1
    root = fixture()
    alias_defs = "".join(f"alias {name}='{value}'; " for name, value in ALIASES.items())
    script = alias_defs + "type -t " + " ".join("'" + n + "'" for n in names)
    want = run(["bash", "-O", "expand_aliases", "-c"], script, root)
    got = run([koa, "-c"], script, root)
    cleanup(root)
    if want.stdout != got.stdout or (want.returncode == 0) != (got.returncode == 0):
        bad.append(f"{script!r}: bash rc={want.returncode} {want.stdout!r}; koa rc={got.returncode} {got.stdout!r}")

for names in [[n] for n in NAMES] + [["cd", "sh"], ["sh", "nonexistent-command-xyz"], ["nonexistent-command-xyz", "cd"], ["ll", "sh", "cd"]]:
    checked += 1
    root = fixture()
    alias_defs = "".join(f"alias {name}='{value}'; " for name, value in ALIASES.items())
    quoted = " ".join("'" + n + "'" for n in names)
    want_kinds, want_paths = [], []
    for name in names:
        kind = run(["bash", "-O", "expand_aliases", "-c"], f"{alias_defs}type -t '{name}'", root).stdout.decode().strip()
        want_kinds.append({"file": "external", "": "missing"}.get(kind, kind))
        path = run(["bash", "-c"], f"type -P '{name}'", root).stdout.decode().strip()
        want_paths.append(path)
    want_rc = run(["bash", "-O", "expand_aliases", "-c"], f"{alias_defs}type {quoted} >/dev/null 2>&1", root).returncode
    got = run([koa, "-c"], f"{alias_defs}type {quoted} | to json", root)
    got_rc = run([koa, "-c"], f"{alias_defs}type {quoted} >/dev/null 2>&1", root).returncode
    cleanup(root)
    try:
        rows = {row["name"]: row for row in json.loads(got.stdout)} if got.stdout.strip() else {}
    except ValueError:
        rows = {}
    for name, kind, path in zip(names, want_kinds, want_paths):
        row = rows.get(name)
        if kind == "missing":
            ok = row is None
        elif kind == "external":
            ok = row is not None and row["kind"] == "external" and row["detail"] == path
        elif kind == "alias":
            ok = row is not None and row["kind"] == "alias" and row["detail"] == ALIASES[name]
        else:
            ok = row is not None and row["kind"] == "builtin"
        if not ok:
            bad.append(f"type {name!r}: bash says {kind} {path!r}, koa says {row}")
    if (want_rc == 0) != (got_rc == 0):
        bad.append(f"type {quoted} exit code: bash {want_rc}, koa {got_rc}")

print(f"checked {checked} directory stack and type cases against bash")
for line in bad:
    print(line[:400])
sys.exit(1 if bad else 0)
