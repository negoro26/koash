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
]
NAMES = [
    "cd", "pwd", "echo", "type", "export", "unset", "alias", "unalias", "history", "jobs", "fg", "bg", "wait",
    "exit", "kill", "pushd", "popd", "dirs",
    "sh", "bash", "sort", "wc", "python3", "sed", "awk", "xargs",
    "nonexistent-command-xyz", "a.b", "", "ll", "gs", "e",
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
    os.symlink("a", os.path.join(root, "link"))
    return root


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
            out = run([koa, "-c"], f"{script}\necho @@; echo \"$PWD|$OLDPWD\"; pwd | to json; dirs | to json", root)
            text = out.stdout.partition(b"@@\n")[2].decode()
            env_line, _, text = text.partition("\n")
            try:
                pwd_rows, dir_rows = (json.loads(blob) for blob in text.replace("]\n[", "]\x00[").split("\x00"))
                cwd = env_line.replace(root, "$ROOT") + " " + pwd_rows[0]["path"].replace(root, "$ROOT")
                rows = [row["path"].replace(root, "$ROOT") for row in dir_rows]
            except (ValueError, IndexError, KeyError):
                cwd, rows = f"unparsable {text[:80]!r}", []
            result = (run([koa, "-c"], script, root).returncode, cwd, rows)
        shutil.rmtree(root)
        results.append(result)
    want, got = results
    if (want[0] == 0) != (got[0] == 0) or want[1:] != got[1:]:
        bad.append(f"{script!r}: bash rc={want[0]} cwd={want[1]} stack={want[2]}; koa rc={got[0]} cwd={got[1]} stack={got[2]}")

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
    shutil.rmtree(root)
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
