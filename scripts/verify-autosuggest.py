#!/usr/bin/env python3
import os
import pty
import select
import shutil
import sys
import tempfile
import time

koa = os.path.abspath(sys.argv[1])
bad = []

RIGHT = b"\x1b[C"
END = b"\x1b[F"
CTRL_E = b"\x05"
ALT_F = b"\x1bf"
TAB = b"\t"
GHOST = "\x1b[90m"
GREEN = "\x1b[32m"
RED = "\x1b[31m"


def run_pty(steps):
    home = tempfile.mkdtemp()
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(home)
        os.environ.update({"HOME": home, "TERM": "xterm", "HISTFILE": "/dev/null"})
        os.execv(koa, [koa, "--norc"])

    buf = bytearray()

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    buf.extend(os.read(fd, 4096))
                except OSError:
                    return

    pump(0.8)
    for data, wait in steps:
        os.write(fd, data)
        pump(wait)
    os.write(fd, b"exit\r")
    pump(0.8)
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)
    os.close(fd)
    files = {}
    for name in os.listdir(home):
        path = os.path.join(home, name)
        if os.path.isfile(path):
            with open(path, "rb") as f:
                files[name] = f.read()
    shutil.rmtree(home, ignore_errors=True)
    return files, bytes(buf).decode("utf-8", "replace")


def case(label, steps, check):
    files, raw = run_pty(steps)
    problem = check(files, raw)
    if problem:
        bad.append(f"{label}: {problem}")


def wrote(name, want):
    return lambda files, raw: None if files.get(name) == want else f"{name}={files.get(name)!r}, want {want!r}"


def absent(name):
    return lambda files, raw: None if name not in files else f"{name} exists: {files[name]!r}"


def both(*checks):
    def run(files, raw):
        for check in checks:
            problem = check(files, raw)
            if problem:
                return problem
        return None

    return run


def shows(text):
    return lambda files, raw: None if text in raw else f"{text!r} was not drawn"


def hides(text):
    return lambda files, raw: None if text not in raw else f"{text!r} was drawn"


HISTORY = [
    (b"echo hello_fish > m1\r", 0.5),
    (b"rm m1\r", 0.4),
    (b"echo hello_", 0.3),
]

case("Right accepts the whole suggestion", [*HISTORY, (RIGHT + b"\r", 0.5)],
     both(wrote("m1", b"hello_fish\n"), shows(f"{GHOST}fish > m1\x1b[0m")))
case("End accepts the whole suggestion", [*HISTORY, (END + b"\r", 0.5)], wrote("m1", b"hello_fish\n"))
case("Ctrl-E accepts the whole suggestion", [*HISTORY, (CTRL_E + b"\r", 0.5)], wrote("m1", b"hello_fish\n"))
case("Enter does not run unaccepted ghost text", [*HISTORY, (b"\r", 0.5)], absent("m1"))
case("Alt-F accepts one word",
     [(b"echo alpha beta > m3\r", 0.5), (b"rm m3\r", 0.4), (b"echo al", 0.3), (ALT_F + b" > m3\r", 0.5)],
     wrote("m3", b"alpha\n"))
case("the most recent matching line is suggested",
     [(b"echo pick_a > m4\r", 0.5), (b"echo pick_b > m4\r", 0.5), (b"rm m4\r", 0.4), (b"echo pick_", 0.3),
      (RIGHT + b"\r", 0.5)],
     wrote("m4", b"pick_b\n"))
case("an empty line has no suggestion",
     [(b"echo hello_empty > m5\r", 0.5), (b"\r", 0.4)], hides(f"{GHOST}echo hello_empty"))

case("an undefined name is red",
     [(b"kzqalias", 0.3)], both(shows(f"{RED}kzqalias"), hides(f"{GREEN}kzqalias")))
case("an alias is green",
     [(b"alias kzqalias='echo ran'\r", 0.5), (b"kzqalias > m6\r", 0.5)],
     both(shows(f"{GREEN}kzqalias"), wrote("m6", b"ran\n")))
case("Tab completes an alias name",
     [(b"alias zzalias='echo tab_ran > m7'\r", 0.5), (b"zzal" + TAB + b"\r", 0.5)], wrote("m7", b"tab_ran\n"))
case("Tab does not invent an alias name", [(b"zzal" + TAB, 0.4)], hides("zzalias"))

SEED = (b"echo x > seed_col\r", 0.5)
case("Tab completes a column after select", [SEED, (b"ls | select na" + TAB + b" > m8\r", 0.6)],
     lambda files, raw: None if b"seed_col" in files.get("m8", b"") else f"m8={files.get('m8')!r}")
case("Tab completes a column after sort-by",
     [(b"echo x > aaa_col\r", 0.5), (b"echo x > zzz_col\r", 0.5),
      (b"ls | sort-by --desc na" + TAB + b" | select name > m9\r", 0.6)],
     wrote("m9", b'{"name":"zzz_col"}\n{"name":"aaa_col"}\n'))
case("Tab completes a column after where",
     [SEED, (b"ls | where na" + TAB + b" == seed_col | select name > m10\r", 0.6)],
     lambda files, raw: None if b"seed_col" in files.get("m10", b"") else f"m10={files.get('m10')!r}")

print("checked 14 interactive suggestion, alias and column completion cases in a pty")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
