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

ERREXIT = [
    "set -e; false; echo after",
    "set -e; true; echo after",
    "set -e; false || echo recovered; echo after",
    "set -e; false && echo x; echo after",
    "set -e; true && false; echo after",
    "set -e; false || false; echo after",
    "set -e; true && true; echo after",
    "set -e; ! false; echo after",
    "set -e; ! true; echo after",
    "set -e; ! true && echo a; echo after",
    "set -e; false | true; echo after",
    "set -e; true | false; echo after",
    "set -e; set +e; false; echo after",
    "set +e; false; echo after",
    "set -e; false; set +e; echo after",
    "set -e; sh -c 'exit 3'; echo after",
    "set -e; sh -c 'exit 3'",
    "set -e; echo a; false; echo b; false; echo c",
    "set -e; cat nonexistent; echo after",
    "set -e; x=1; echo $x",
    "set -e\nfalse\necho after",
    "set -e\ntrue\necho after",
    "set -e; echo a &\nwait; echo after",
    "set -o errexit; false; echo after",
    "set -o errexit; set +o errexit; false; echo after",
    "set -e; if true; then false; echo in; fi; echo after",
    "set -e\nif true; then false; echo in; fi\necho after",
    "set -e\nfor i in 1 2; do false; echo in; done\necho after",
    "set -e\n(false; echo in)\necho after",
    "set -e\nf() { false; echo in; }\nf\necho after",
    "set -e\nfalse || true\necho after",
    "set -e\nwhile false; do :; done\necho after",
]
PIPEFAIL = [
    "set -o pipefail; false | true; echo $?",
    "set -o pipefail; true | false | true; echo $?",
    "set -o pipefail; sh -c 'exit 3' | sh -c 'exit 2' | true; echo $?",
    "set -o pipefail; sh -c 'exit 3' | sh -c 'exit 2' | sh -c 'exit 0'; echo $?",
    "set -o pipefail; true | true; echo $?",
    "false | true; echo $?",
    "set -o pipefail; set +o pipefail; false | true; echo $?",
    "set -o pipefail; false | cat; echo $?",
    "set -o pipefail; echo a | false; echo $?",
    "set -o pipefail; echo a | false | echo b; echo $?",
    "set -o pipefail; ! false | true; echo $?",
    "set -o pipefail; ! true | true; echo $?",
    "set -o pipefail; false | true && echo ok || echo failed",
    "set -eo pipefail; false | true; echo after",
    "set -e -o pipefail; false | true; echo after",
    "set -e; set -o pipefail; true | false; echo after",
    "set -o pipefail; cat nonexistent | cat; echo $?",
]
PIPESTATUS = [
    "false | true; echo $PIPESTATUS",
    "true | false; echo $PIPESTATUS",
    "sh -c 'exit 3' | sh -c 'exit 2' | true; echo $PIPESTATUS",
    "false; echo $PIPESTATUS",
    "true; echo $PIPESTATUS",
    "echo a | cat | cat; echo $PIPESTATUS",
    "sh -c 'exit 4'; echo $PIPESTATUS",
    "false | true; true; echo $PIPESTATUS",
    "! false | true; echo $PIPESTATUS",
    "set -o pipefail; false | true; echo $PIPESTATUS",
    "cat nonexistent | cat; echo $PIPESTATUS",
    "echo a | sh -c 'exit 5'; echo $PIPESTATUS",
    "false | true; sh -c 'echo [$PIPESTATUS]'",
    "false | true; sh -c 'env | grep -c PIPESTATUS'",
    "sh -c 'exit 3' | true; PIPESTATUS_PROBE=1 sh -c 'echo [$PIPESTATUS]'",
]
SETFLAGS = [
    "set -euo pipefail; echo hi",
    "set -eu; echo hi",
    "set -ue; echo hi",
    "set -e -u; echo hi",
    "set -eu -o pipefail; echo hi",
    "set -o errexit -o pipefail; false | true; echo after",
    "set -euo pipefail; false | true; echo after",
    "set -euo pipefail; true | false; echo after",
    "set -euo pipefail; set +eu +o pipefail; false | true; echo after",
    "set -e -Z; echo $?; false; echo after",
    "set -Z; echo $?",
    "set -o nonsuch; echo $?",
    "set -o nonsuch; echo $?; false; echo after",
    "set -ex; echo hi",
    "set -x; echo hi",
    "set -C; echo hi",
    "set -euxo pipefail; false | true; echo after",
    "set -o posix; echo hi",
    "set -euo pipefail; false; echo after",
    "set -u; echo $NOPE; echo after",
    "set -u; echo ${NOPE:-d}",
    "set -u; echo ${NOPE:-d}; echo $NOPE; echo after",
    "set -eu; echo \"$@\" x",
    "set -u; v=1; echo ${v:-d} ${v-d}",
    "set -o allexport; echo hi; echo $?",
]
NOUNSET = [
    "set -u; echo a; echo $x; echo b",
    "set -u; echo $x; echo after",
    "set -u; echo \"$x\"; echo after",
    "set -u; echo ok; echo $@ $* $?; echo after",
    "set -u; false && echo $x; echo after",
    "set -u; true || echo $x; echo after",
    "set -u; x=$y; echo after",
    "set -u; x=1; echo $x",
    "set -u; x=1; unset x; echo $x; echo after",
    "set -u; export X=1; echo $X",
    "set -u; echo $x || echo y; echo after",
    "set -u; set +u; echo $x ok",
    "set -u; set +o nounset; echo $x ok",
    "set -o nounset; echo $x; echo after",
    "set -u; cat $x </dev/null; echo after",
    "set -u; echo a > $x; echo after",
    "set -u; echo $HOME > /dev/null; echo ok",
    "set -eu; echo $x; echo after",
    "set -u; echo $x | cat; echo after",
    "set -u; echo ~; echo after",
]
NEGATION = [
    "! true; echo $?",
    "! false; echo $?",
    "! sh -c 'exit 3'; echo $?",
    "! echo ok; echo $?",
    "! true && echo yes || echo no",
    "! false && echo yes || echo no",
    "! true | false; echo $?",
    "! false | true; echo $?",
    "! nonexistent-command-xyz; echo $?",
    "! cat nonexistent; echo $?",
    "!true; echo $?",
    "echo a ! b",
    "echo !",
    "false; ! true; echo $?",
    "! true; ! false; echo $?",
    "! cd /; echo $?",
    "! cd /nonexistent-dir-xyz 2>/dev/null; echo $?",
    "! export NEGATED=1; echo $?",
    "! unset NEGATED; echo $?",
    "set -e; ! cd /nonexistent-dir-xyz 2>/dev/null; echo after",
    "! ! false; echo $?",
    "! ! true; echo $?",
    "! ! ! true; echo $?",
    "! ! ! false; echo $?",
    "!; echo next",
    "!; echo $?",
    "! !; echo $?",
    "!\necho after $?",
    "false; !; echo $?",
    "set -e; ! ! false; echo after",
    "set -e; ! ! true; echo after",
    "set -e; !; echo after",
]
NOGLOB = [
    "set -f; printf '[%s]' *.txt",
    "set -f; printf '[%s]' *",
    "set -f; set +f; printf '[%s]' *.txt",
    "set -o noglob; printf '[%s]' *.txt",
    "set -o noglob; set +o noglob; printf '[%s]' *.txt",
    "set -f; printf '[%s]' \"*.txt\"",
    "set -f; printf '[%s]' ?.txt [ab].txt a.{txt,log}",
    "set -f; echo *.txt > out; sh -c 'cat out'",
    "set -f; printf '[%s]' *.txt; set +f; printf '[%s]' *.txt",
    "set +f; printf '[%s]' *.txt",
    "set -f; printf '[%s]' sub/*",
    "printf '[%s]' *.txt",
    "set -ef; printf '[%s]' *.txt; false; echo after",
    "set -fe; printf '[%s]' *.txt; false; echo after",
    "set -f; echo hi > x*.out; printf '[%s]' *.out",
]
OPTION_LISTING = [
    "",
    "set -e",
    "set -f",
    "set -o pipefail",
    "set -e; set -f; set -o pipefail; set +e",
    "set -eo pipefail",
    "set -ef",
    "set -e; set +o errexit",
    "set -o noglob; set +f",
]
BAD_OPTIONS = ["set -o bogus", "set -o", "set -Z"]


def run(argv, script, cwd):
    try:
        return subprocess.run(argv + [script], cwd=cwd, env=ENV, capture_output=True, timeout=20)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, -9, b"", b"timeout")


def fixture():
    root = tempfile.mkdtemp()
    for name in ("a.txt", "b.txt", "c.log"):
        open(os.path.join(root, name), "w").close()
    os.mkdir(os.path.join(root, "sub"))
    open(os.path.join(root, "sub", "in.txt"), "w").close()
    return root


def compare(script, bash_script=None):
    global checked
    checked += 1
    results = []
    for argv, text in (([ "bash", "-c"], bash_script or script), ([koa, "-c"], script)):
        root = fixture()
        out = run(argv, text, root)
        results.append((out, sorted(os.listdir(root))))
        shutil.rmtree(root)
    (want, files_want), (got, files_got) = results
    if want.stdout != got.stdout or want.returncode != got.returncode or files_want != files_got:
        bad.append(
            f"{script!r}: bash rc={want.returncode} {want.stdout!r}, koa rc={got.returncode} {got.stdout!r} {got.stderr[:80]!r}"
        )


for script in ERREXIT + PIPEFAIL + NEGATION + NOGLOB:
    compare(script)
for script in SETFLAGS + NOUNSET:
    compare(script)
for script in PIPESTATUS:
    compare(script, script.replace("$PIPESTATUS", '"${PIPESTATUS[*]}"').replace("echo ", "echo ", 1))

STDIN_SCRIPTS = [
    "set -e\nif true; then false; echo in; fi\necho after\n",
    "set -e\nfalse\necho after\n",
    "set -e\ntrue\nif true; then echo in; fi\necho after\n",
    "set -e\nset +e\nfalse\nif true; then echo in; fi\necho after\n",
    "set -e\nfalse || true\nif true; then echo in; fi\necho after\n",
    "set -e\nfalse && true\necho after\n",
    "set -e\nexport x=$(false; echo hi)\necho after $x\n",
    "set -o pipefail\nfor i in 1; do false | true; echo $?; done\n",
    "set -o pipefail\nfalse | true\necho $?\n",
    "set -eo pipefail\nfalse | true\necho after\n",
    "set -eo pipefail\nfor i in 1; do true | false; echo in; done\necho after\n",
    "set -f\nfor i in *.txt; do echo $i; done\n",
    "set -f\nset +f\nfor i in *.txt; do echo $i; done\n",
    "set -e\nexit 3\n",
    "set -e\nsh -c 'exit 7'\necho after\n",
]
for script in STDIN_SCRIPTS:
    checked += 1
    results = []
    for argv in (["bash"], [koa]):
        root = fixture()
        out = subprocess.run(argv, input=script.encode(), cwd=root, env=ENV, capture_output=True, timeout=20)
        results.append(out)
        shutil.rmtree(root)
    want, got = results
    if want.stdout != got.stdout or want.returncode != got.returncode:
        bad.append(f"stdin {script!r}: bash rc={want.returncode} {want.stdout!r}, koa rc={got.returncode} {got.stdout!r}")

for prefix in OPTION_LISTING:
    checked += 1
    root = fixture()
    sep = "; " if prefix else ""
    want = run(["bash", "-c"], f"{prefix}{sep}set -o | grep -E '^(errexit|noglob|nounset|pipefail)'", root).stdout.decode()
    want = {line.split()[0]: line.split()[1] == "on" for line in want.splitlines()}
    got = run([koa, "-c"], f"{prefix}{sep}set -o | to json", root)
    try:
        got = {row["option"]: row["enabled"] for row in json.loads(got.stdout)}
    except ValueError:
        got = {"unparsable": got.stderr[:80]}
    shutil.rmtree(root)
    if want != got:
        bad.append(f"{prefix!r} then set -o: bash {want}, koa {got}")

for script in BAD_OPTIONS:
    checked += 1
    root = fixture()
    want = run(["bash", "-c"], script, root).returncode
    got = run([koa, "-c"], script, root).returncode
    shutil.rmtree(root)
    if (want == 0) != (got == 0):
        bad.append(f"{script!r}: bash rc={want}, koa rc={got}")

NOTICES = [
    ("set -x; echo hi", "xtrace"),
    ("set -ex; echo hi", "xtrace"),
    ("set -o monitor; echo hi", "monitor"),
    ("set -euxo pipefail; echo hi", "xtrace"),
]
for script, name in NOTICES:
    checked += 1
    root = fixture()
    got = run([koa, "-c"], script, root)
    shutil.rmtree(root)
    lines = [l for l in got.stderr.decode().splitlines() if name in l]
    if got.returncode != 0 or got.stdout != b"hi\n" or len(lines) != 1 or len(got.stderr.splitlines()) != 1:
        bad.append(f"{script!r}: want rc 0, stdout 'hi' and one notice naming {name}; got rc={got.returncode} {got.stdout!r} {got.stderr[:120]!r}")

print(f"checked {checked} scripts against bash")
for line in bad:
    print(line[:400])
sys.exit(1 if bad else 0)
