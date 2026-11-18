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


def leader_ctrl_c(line):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(work)
        os.execv(koa, [koa, "-c", line])
    time.sleep(1.0)
    os.write(fd, b"\x03")
    time.sleep(0.5)
    os.close(fd)
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass
    time.sleep(0.5)


def normal_exit(line):
    subprocess.run([koa, "-c", line], cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)


def nohup_hangup(line):
    proc = subprocess.Popen(
        ["/usr/bin/nohup", koa, "-c", line], cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    time.sleep(0.5)
    proc.send_signal(signal.SIGHUP)
    time.sleep(0.5)


cases = []
for job in ("/bin/sleep {t} &", "sleep {t} | ls > x.txt &"):
    for label, run, wrap in (
        ("terminal closed", close_terminal, lambda s: f"{koa} -c '{s}'"),
        ("SIGTERM to koa -c", sigterm, lambda s: s),
        ("Ctrl-C, koa -c leads the session", leader_ctrl_c, lambda s: s),
    ):
        t, f = tag(), tag()
        cases.append((label, run, wrap(f"{job.format(t=t)} /bin/sleep {f}"), t, f, False))
t, f = tag(), tag()
cases.append(("normal exit", normal_exit, f"/bin/sleep {t} & /bin/sleep 0.{f.replace('.', '')}", t, f, True))
t, f = tag(), tag()
cases.append(("SIGHUP under nohup", nohup_hangup, f"/bin/sleep {t} & /bin/sleep {f}", t, f, True))

for label, run, line, t, f, keep in cases:
    run(line)
    left = running(t)
    running(f)
    if bool(left) != keep:
        want = "keep running, as bash -c" if keep else "be ended, as bash does"
        bad.append(f"{label}: {line!r}: the background job must {want}, left running: {left}")

shutil.rmtree(work, ignore_errors=True)
print(f"checked {len(cases)} hangup cases")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
