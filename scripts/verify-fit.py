#!/usr/bin/env python3
import fcntl
import json
import os
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata
import pty

koa = os.path.abspath(sys.argv[1])
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
WIDTHS = [40, 80, 120]
LONG = "an-extraordinarily-long-file-name-that-goes-on-and-on-and-on-forever-and-ever-amen.txt"
CJK = "日本語のとても長いファイル名がここにあります日本語のとても長いファイル名がここにあります.md"
EMOJI = "😀emoji-😀-name-😀-and-a-lot-more-😀-text-to-be-sure-it-is-cut-😀.txt"


def width(text):
    total = 0
    for ch in text:
        if unicodedata.category(ch) in ("Cc", "Cf", "Mn", "Me"):
            continue
        total += 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1
    return total


def spawn(argv, cols, cwd, env_extra, stdout_fd=None):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(cwd)
        if stdout_fd is not None:
            os.dup2(stdout_fd, 1)
        env = {k: v for k, v in os.environ.items() if k not in ("COLUMNS", "TERM_PROGRAM")}
        env.update({"TERM": "xterm-256color", "HOME": cwd, **env_extra})
        os.execve(argv[0], argv, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, cols, 0, 0))
    return pid, fd


def drain(fd, quiet, limit):
    out = b""
    end = time.time() + limit
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], quiet)
        if not ready:
            return out, True
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            return out, False
        if not chunk:
            return out, False
        out += chunk
    return out, False


def reap(pid):
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)


def clean(raw):
    return ANSI.sub("", raw.decode(errors="replace").replace("\r\n", "\n"))


def on_tty(script, cols, cwd, env_extra=None):
    pid, fd = spawn([koa, "-c", script], cols, cwd, env_extra or {})
    out, _ = drain(fd, 5, 30)
    reap(pid)
    return clean(out)


def on_split_tty(script, control_cols, stdout_cols, cwd):
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, stdout_cols, 0, 0))
    pid, fd = spawn([koa, "-c", script], control_cols, cwd, {}, stdout_fd=slave)
    os.close(slave)
    out, _ = drain(master, 5, 30)
    reap(pid)
    os.close(master)
    os.close(fd)
    return clean(out)


def repl_once(script, cols, cwd, env_extra=None, before=(), resize=None):
    pid, fd = spawn([koa], cols, cwd, env_extra or {})
    drain(fd, 1.2, 15)
    for line in before:
        os.write(fd, line.encode() + b"\r")
        drain(fd, 2, 15)
    if resize:
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, resize, 0, 0))
    os.write(fd, script.encode() + b"\r")
    out, _ = drain(fd, 1.2, 15)
    reap(pid)
    lines = clean(out).split("\n")
    typed = max((i for i, line in enumerate(lines) if "koa>" in line and line.rstrip().endswith(script)), default=-1)
    return "\n".join(line for line in lines[typed + 1 :] if "koa>" not in line)


def on_tty_repl(*args, **kwargs):
    # a loaded machine can make the line editor answer after the quiet period; only an empty read is retried
    for _ in range(3):
        out = repl_once(*args, **kwargs)
        if out.strip():
            break
    return out


def piped(script, cwd):
    return subprocess.run([koa, "-c", script], cwd=cwd, capture_output=True, text=True).stdout


def widest(text):
    return max((width(line) for line in text.split("\n")), default=0)


root = tempfile.mkdtemp()
for name in ("a.txt", "b.txt", LONG, CJK, EMOJI):
    open(os.path.join(root, name), "w").close()

bad = []
checks = 0


def expect(ok, message):
    global checks
    checks += 1
    if not ok:
        bad.append(message)


grep_row = json.loads(piped("help grep | to json", root))[0]
flags = grep_row["flags"].split()
description_words = grep_row["description"].split()

for cols in WIDTHS:
    for script in ["help", "help grep", "ls", "ps", "env", "help | select command flags"]:
        out = on_tty(script, cols, root)
        expect(out.strip() != "", f"{cols} cols {script!r}: no output")
        expect(widest(out) <= cols, f"{cols} cols {script!r}: widest line is {widest(out)}")

    out = on_tty("help", cols, root)
    expect("description" in out.split("\n")[0], f"{cols} cols 'help': no description column")
    expect("--ignore-case" not in out, f"{cols} cols 'help': prints flags")
    expect("grep" in out and "sort" in out, f"{cols} cols 'help': commands missing")

    tokens = on_tty("help grep", cols, root).split()
    for word in [*flags, *description_words]:
        expect(word in tokens, f"{cols} cols 'help grep': {word!r} missing")

    out = on_tty("ls", cols, root)
    expect("…" in out, f"{cols} cols 'ls': no cut cell")
    expect("a.txt" in out and "b.txt" in out, f"{cols} cols 'ls': short names cut")
    expect(LONG not in out and CJK not in out, f"{cols} cols 'ls': long names not cut")

out = on_tty("ls", 120, root, {"COLUMNS": "60"})
expect(widest(out) <= 60, f"COLUMNS=60 on 120 cols: widest line is {widest(out)}")
out = on_tty("ls", 80, root, {"COLUMNS": "200"})
expect(80 < widest(out) <= 200 and LONG in out, f"COLUMNS=200 on 80 cols: widest line is {widest(out)}")

out = on_tty_repl("ls", 80, root)
expect("…" in out and widest(out) <= 80, f"repl at 80 cols: widest line is {widest(out)}")
out = on_tty_repl("help", 40, root)
expect("description" in out and widest(out) <= 40, f"repl at 40 cols: widest line is {widest(out)}")

out = on_tty("export COLUMNS=40; ls", 80, root)
expect(widest(out) <= 40 and "…" in out, f"export COLUMNS=40 in the session: widest line is {widest(out)}")
out = on_tty("COLUMNS=40 ls", 80, root)
expect(widest(out) <= 40 and "…" in out, f"COLUMNS=40 prefix: widest line is {widest(out)}")
out = on_tty("COLUMNS=40 ls", 80, root, {"COLUMNS": "100"})
expect(widest(out) <= 40, f"prefix COLUMNS over inherited 100: widest line is {widest(out)}")
out = on_tty_repl("ls", 80, root, {"COLUMNS": "100"})
expect(widest(out) <= 100 and widest(out) > 80, f"inherited COLUMNS=100 in the repl: widest line is {widest(out)}")
out = on_tty_repl("ls", 80, root, {"COLUMNS": "100"}, before=["unset COLUMNS"], resize=40)
expect(0 < widest(out) <= 40, f"unset COLUMNS then resize to 40: widest line is {widest(out)}")

out = on_split_tty("ls", 100, 40, root)
expect(0 < widest(out) <= 40 and "…" in out, f"stdout 40 cols, controlling terminal 100: widest line is {widest(out)}")
out = on_tty("ls", 0, root)
expect(0 < widest(out) <= 80 and "…" in out, f"terminal reporting 0 columns: widest line is {widest(out)}")

one = tempfile.mkdtemp()
open(os.path.join(one, CJK), "w").close()
for cols in [1, 2, 3, 5, 10]:
    for script, cwd in [("ls", root), ("ls", one), ("help grep", root), ("help", root)]:
        out = on_tty(script, cols, cwd)
        expect(out.strip() != "" and widest(out) <= cols, f"{cols} cols {script!r}: widest line is {widest(out)}")
    out = on_tty("ls", cols, root, {"TERM_PROGRAM": "Tern"})
    preview = re.sub(r"\x1b\]9001;.*?\x07", "", out)
    expect(widest(preview) <= cols, f"{cols} cols tern preview: widest line is {widest(preview)}")

out = on_tty("@sql SELECT 1.0 AS x, 2.5 AS y", 80, root, {"TERM_PROGRAM": "Tern"})
preview = re.sub(r"\x1b\]9001;.*?\x07", "", out)
expect("1.0" in preview and "2.5" in preview, "tern preview rewrote a float cell")

out = piped("help", root)
expect("--files-with-matches" in out and widest(out) > 200, "piped help must keep every flag")
out = piped("ls", root)
expect(LONG in out and CJK in out and "…" not in out, "piped ls must not cut")
expect(
    sorted(row["name"] for row in json.loads(piped("ls | to json", root)))
    == sorted(["a.txt", "b.txt", LONG, CJK, EMOJI]),
    "ls | to json lost a name",
)

shutil.rmtree(root)
shutil.rmtree(one)
print(f"checked {checks} terminal fit forms")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
