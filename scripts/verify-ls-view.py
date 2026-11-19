#!/usr/bin/env python3
import os
import pty
import re
import select
import shutil
import subprocess
import sys
import tempfile
import time

koa = os.path.abspath(sys.argv[1])
bad = []


def terminal(line, view, path=None):
    home = tempfile.mkdtemp()
    open(os.path.join(home, "a.txt"), "w").close()
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(home)
        env = {k: v for k, v in os.environ.items() if k != "KOA_LS_VIEW"}
        env.update({"HOME": home, "TERM": "dumb"})
        if path is not None:
            env["PATH"] = path
        if view is not None:
            env["KOA_LS_VIEW"] = view
        os.execve(koa, [koa, "-c", line], env)
    out = b""
    end = time.time() + 5
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if ready:
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
    os.waitpid(pid, 0)
    os.close(fd)
    shutil.rmtree(home, ignore_errors=True)
    return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", out.decode(errors="replace")).replace("\r", "")


VIEW = "/bin/echo VIEW"
fake = tempfile.mkdtemp()
with open(os.path.join(fake, "eza"), "w") as fh:
    fh.write("#!/bin/sh\necho EZA \"$@\"\n")
os.chmod(os.path.join(fake, "eza"), 0o755)
empty = tempfile.mkdtemp()
cases = [
    ("bare ls runs the view", "ls", VIEW, None, lambda o: o.strip() == "VIEW"),
    ("ls with paths passes them on", "ls a.txt .", VIEW, None, lambda o: o.strip() == "VIEW a.txt ."),
    ("an option keeps koa's ls", "ls -a", VIEW, None, lambda o: "VIEW" not in o and "a.txt" in o),
    ("a pipe keeps koa's ls", "ls | select name", VIEW, None, lambda o: "VIEW" not in o and "a.txt" in o),
    ("a redirect keeps koa's ls", "ls > out.txt; cat --raw out.txt", VIEW, None, lambda o: "VIEW" not in o and "a.txt" in o),
    ("a missing view program keeps koa's ls", "ls", "/nonexistent/view -l", None, lambda o: "a.txt" in o and "not found" not in o),
    (
        "unset with eza installed runs eza with the Omarchy flags",
        "ls",
        None,
        fake,
        lambda o: o.strip() == "EZA -lh --group-directories-first --icons=auto",
    ),
    ("unset without eza keeps koa's ls", "ls", None, empty, lambda o: "a.txt" in o and "EZA" not in o),
    ("an empty setting turns the view off", "ls", "", fake, lambda o: "a.txt" in o and "EZA" not in o),
]
for label, line, view, path, ok in cases:
    out = terminal(line, view, path)
    if not ok(out):
        bad.append(f"{label}: koa -c {line!r} printed {out.strip()[:200]!r}")
shutil.rmtree(fake, ignore_errors=True)
shutil.rmtree(empty, ignore_errors=True)

piped = subprocess.run(
    [koa, "-c", "ls"], capture_output=True, text=True, env={**os.environ, "KOA_LS_VIEW": VIEW}, cwd=tempfile.gettempdir()
)
if "VIEW" in piped.stdout:
    bad.append(f"stdout not a terminal must keep koa's ls, got {piped.stdout[:200]!r}")

print(f"checked {len(cases) + 1} ls view cases")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
