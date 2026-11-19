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


def terminal(line, view):
    home = tempfile.mkdtemp()
    open(os.path.join(home, "a.txt"), "w").close()
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(home)
        env = {k: v for k, v in os.environ.items() if k != "KOA_LS_VIEW"}
        env.update({"HOME": home, "TERM": "dumb"})
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
cases = [
    ("bare ls runs the view", "ls", VIEW, lambda o: o.strip() == "VIEW"),
    ("ls with paths passes them on", "ls a.txt .", VIEW, lambda o: o.strip() == "VIEW a.txt ."),
    ("an option keeps koa's ls", "ls -a", VIEW, lambda o: "VIEW" not in o and "a.txt" in o),
    ("a pipe keeps koa's ls", "ls | select name", VIEW, lambda o: "VIEW" not in o and "a.txt" in o),
    ("a redirect keeps koa's ls", "ls > out.txt; cat --raw out.txt", VIEW, lambda o: "VIEW" not in o and "a.txt" in o),
    ("no setting keeps koa's ls", "ls", None, lambda o: "VIEW" not in o and "a.txt" in o),
    ("a missing view program keeps koa's ls", "ls", "/nonexistent/view -l", lambda o: "a.txt" in o and "not found" not in o),
]
for label, line, view, ok in cases:
    out = terminal(line, view)
    if not ok(out):
        bad.append(f"{label}: koa -c {line!r} printed {out.strip()[:200]!r}")

piped = subprocess.run(
    [koa, "-c", "ls"], capture_output=True, text=True, env={**os.environ, "KOA_LS_VIEW": VIEW}, cwd=tempfile.gettempdir()
)
if "VIEW" in piped.stdout:
    bad.append(f"stdout not a terminal must keep koa's ls, got {piped.stdout[:200]!r}")

print(f"checked {len(cases) + 1} ls view cases")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
