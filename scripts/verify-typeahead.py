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


def session(steps):
    home = tempfile.mkdtemp()
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(home)
        os.environ.update({"HOME": home, "TERM": "xterm"})
        os.execv(koa, [koa])

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    os.read(fd, 4096)
                except OSError:
                    return

    pump(1.0)
    for data, wait in steps:
        os.write(fd, data)
        pump(wait)
    os.write(fd, b"exit\r")
    pump(1.0)
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)
    os.close(fd)
    files = set(os.listdir(home))
    shutil.rmtree(home, ignore_errors=True)
    return files


cases = [
    ("line typed while a command runs", [(b"/bin/sleep 1\r", 0.3), (b"/usr/bin/touch ahead\r", 2.0)], "ahead"),
    ("two lines typed while a command runs", [(b"/bin/sleep 1\r", 0.3), (b"/usr/bin/touch one\r/usr/bin/touch two\r", 2.0)], "two"),
    ("Ctrl-J ends a line, as in bash", [(b"/usr/bin/touch ctrlj\n", 1.0)], "ctrlj"),
    (
        "Ctrl-J ends Ctrl-R search and keeps the line to edit, as in bash",
        [(b"/usr/bin/touch first\r", 0.6), (b"\x12first", 0.4), (b"\n", 0.3), (b"\x05X\r", 1.0)],
        "firstX",
    ),
]
for label, steps, want in cases:
    files = session(steps)
    if want not in files:
        bad.append(f"{label}: {want!r} was not created, files: {sorted(files)}")

print(f"checked {len(cases)} typed-ahead cases")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
