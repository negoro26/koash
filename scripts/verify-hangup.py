#!/usr/bin/env python3
import os
import pty
import random
import shutil
import signal
import subprocess
import sys
import tempfile
import time

koa = os.path.abspath(sys.argv[1])
work = tempfile.mkdtemp()
bad = []


def running(tag):
    out = subprocess.run(["pgrep", "-f", f"sleep {tag}$"], capture_output=True, text=True).stdout.split()
    for pid in out:
        os.kill(int(pid), signal.SIGKILL)
    return out


def tag():
    return f"{random.randint(30, 89)}.{random.randint(100000, 999999)}"


def close_terminal(line):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(work)
        os.execvp("bash", ["bash", "--norc", "--noprofile", "-i"])
    time.sleep(0.5)
    os.write(fd, (line + "\r").encode())
    time.sleep(1.0)
    os.close(fd)
    time.sleep(1.0)
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass


def sigterm(line):
    proc = subprocess.Popen([koa, "-c", line], cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.0)
    proc.send_signal(signal.SIGTERM)
    proc.wait()
    time.sleep(0.5)


def normal_exit(line):
    subprocess.run([koa, "-c", line], cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)


cases = []
for job in ("/bin/sleep {t} &", "sleep {t} | ls > x.txt &"):
    t = tag()
    cases.append(("terminal closed", close_terminal, f"{koa} -c '{job.format(t=t)} /bin/sleep {tag()}'", t, False))
    t = tag()
    cases.append(("SIGTERM to koa -c", sigterm, f"{job.format(t=t)} /bin/sleep {tag()}", t, False))
t = tag()
cases.append(("normal exit", normal_exit, f"/bin/sleep {t} &", t, True))

for label, run, line, t, keep in cases:
    run(line)
    left = running(t)
    if bool(left) != keep:
        want = "keep running, as bash -c" if keep else "be ended, as bash does"
        bad.append(f"{label}: {line!r}: the background job must {want}, left running: {left}")

shutil.rmtree(work, ignore_errors=True)
print(f"checked {len(cases)} hangup cases")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
