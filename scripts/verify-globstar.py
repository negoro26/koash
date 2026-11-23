#!/usr/bin/env python3
import os
import shutil
import socket
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
bad = []
checked = 0
ENV = {"PATH": os.environ["PATH"], "LC_ALL": "C", "HOME": "/nonexistent"}

FILES = [
    "root.rs", "root.txt", "Upper.RS", ".hidden.rs", "a/one.rs", "a/two.txt", "a/b/c/deep.rs", "a/b/mid.rs",
    "a/b/c/d/e/deepest.rs", "a/.dot/secret.rs", ".hid/in.rs", "sp ace/in side.rs", "sp ace/sub dir/x.rs",
    "\u00e9t\u00e9/\u00fcn\u00ef.rs", "\u65e5\u672c/\u8a9e.txt", "z/x.rs", "z/y/x.rs", "x.rs", "dir.rs/inner.rs",
    "dir.rs/deeper/inner.rs", "star*/q.rs", "-dash/f.rs", "a1/f.rs", "a2/f.rs", "b/f.rs", "b/g/f.rs",
]
DIRS = ["empty", "a/emptydir", "z/y/emptyleaf"]
LINKS = {
    "link-to-a": "a",
    "a/b/loop": "../..",
    "a/self": ".",
    "link-to-file": "root.rs",
    "broken": "nowhere",
    "z/to-b": "../b",
    "z/up": "..",
    "chain1": "chain2",
    "chain2": "chain1",
    "a/to-root-file": "../root.rs",
}
PATTERNS = [
    "**", "**/", "**/*", "**/*/", "**/*.rs", "**/*.txt", "**/*.RS", "**/.*", "**/.*.rs", "**/.hid*", ".*/**", ".*/**/*.rs",
    "a/**", "a/**/", "a/**/*", "a/**/*.rs", "a/**/**/*.rs", "a/b/**", "a/b/**/", "a/b/**/*.rs", "a/**/deep.rs",
    "a/**/c/**/*.rs", "**/b/*.rs", "**/b/**/*.rs", "**/c/*.rs", "**/*/*.rs", "**/**/*.rs", "*/**/*.rs", "*/**",
    "**/x.rs", "z/**/x.rs", "z/**/y/x.rs", "**/y/x.rs", "./**/*.rs", "./**", "./a/**/*.rs", "**/f.rs", "a?/**/f.rs",
    "a*/f.rs", "**/[ab]*.rs", "**/?.rs", "**/??.rs", "**/o*e.rs", "**/sub dir/*.rs", "sp*/**/*.rs", "\"sp ace\"/**/*.rs",
    "sp\\ ace/**/*.rs", "**/\u00fcn\u00ef.rs", "\u00e9t\u00e9/**", "**/*.nomatch", "nomatch/**/*.rs", "a/nomatch/**",
    "'**'/*.rs", "\"**\"", "\\*\\*/*.rs", "**/*.rs **/*.txt", "**/*.rs *.txt", "root.rs **/x.rs", "**/*.nomatch *.rs",
    "a**/f.rs", "**a/f.rs", "a/**b", "a/b**", "**b/*", "a/***/*.rs", "***", "***/*.rs", "**/*.r?", "**/*.[rt]*",
    "-dash/**", "star*/**", "'star*'/**", "a/b/../**", "a/b/c/../**/*.rs", "a/../**/mid.rs",
    "**/dir.rs", "**/dir.rs/", "dir.rs/**", "**/*/inner.rs", "empty/**", "empty/**/", "**/empty", "**/empty/",
    "**/emptydir/**", "link-to-a/**", "link-to-a/**/*.rs", "link-to-a/", "**/link*", "**/link*/", "**/self/**",
    "**/loop/**", "**/to-b/**", "**/to-b/*.rs", "z/to-b/**", "z/up/**", "**/up/*", "**/broken", "**/chain*",
    "**/*-file", "**/to-root-file", "**/f.rs/", "**/*.rs/", "**/*.RS/", "**/**", "**/**/", "a/**/**/", "**/a*/**/*.rs", "**/sub dir/**", "**/c/**", "**/d/**/", "sp*/**/**/*.rs", "**/**/**/*.rs",
    "**/*.rs/**", "**/z/**/x.rs", "z/**/z", "**/emptyleaf/**/", "./**/", "a/./**/*.rs", "a//**/*.rs",
]
SCRIPTS = (
    [f"printf '[%s]\\n' {p}" for p in PATTERNS]
    + [
        "echo **/*.rs",
        "echo a/**/*.rs | wc -w",
        "cd a; printf '[%s]\\n' **/*.rs",
        "cd a/b; printf '[%s]\\n' **",
        "cd a/b; printf '[%s]\\n' ../**/*.rs",
        "cd z; printf '[%s]\\n' **/x.rs",
        "x=a; printf '[%s]\\n' $x/**/*.rs",
        "printf '[%s]\\n' $PWD/**/*.rs",
        "printf '[%s]\\n' $PWD/a/**",
        "printf '[%s]\\n' /nonexistent-dir/**",
        "echo '**/*.rs'",
        "echo \"**/*.rs\"",
        "echo **/*.rs > out.txt; sh -c 'cat out.txt'",
        "printf '[%s]\\n' **/*.rs > /dev/null; echo $?",
        "FOO=**/x.rs; echo $FOO",
        "echo {a,z}/**/*.rs",
        "echo a/**/{one,mid}.rs",
        "for f in a/**/*.rs; do echo $f; done",
    ]
)


def build(root):
    for path in FILES:
        full = os.path.join(root, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        open(full, "w").close()
    for path in DIRS:
        os.makedirs(os.path.join(root, path), exist_ok=True)
    for path, target in LINKS.items():
        os.symlink(target, os.path.join(root, path))
    sock = socket.socket(socket.AF_UNIX)
    sock.bind(os.path.join(root, "a", "sock.rs"))
    sock.close()


def run(argv, script, cwd):
    try:
        return subprocess.run(
            argv + [script], cwd=cwd, env={**ENV, "PWD": cwd}, capture_output=True, timeout=30
        )
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, -9, b"", b"timeout")


root = os.path.realpath(tempfile.mkdtemp())
build(root)
for script in SCRIPTS:
    checked += 1
    want = run(["bash", "-O", "globstar", "-c"], script, root)
    got = run([koa, "-c"], script, root)
    if want.stdout != got.stdout or (want.returncode == 0) != (got.returncode == 0):
        bad.append(
            f"{script!r}:\n  bash rc={want.returncode} {want.stdout.decode(errors='replace')!r}\n"
            f"  koa  rc={got.returncode} {got.stdout.decode(errors='replace')!r}"
        )
shutil.rmtree(root)

print(f"checked {checked} globstar scripts against bash")
for line in bad:
    print(line[:700])
sys.exit(1 if bad else 0)
