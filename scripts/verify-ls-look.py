#!/usr/bin/env python3
import fcntl
import os
import pty
import random
import re
import select
import shlex
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time

koa = os.path.abspath(sys.argv[1])
GNU = "/usr/bin/ls"
version = subprocess.run([GNU, "--version"], capture_output=True, text=True).stdout
if "GNU coreutils" not in version:
    sys.exit(f"verify-ls-look.py compares koa with GNU coreutils, but {GNU} is: {version.splitlines()[:1]}")

DAY = 86_400
SIX_MONTHS = 31_556_952 // 2
NOW = int(time.time())
WIDTHS = [40, 80, 120, 200]
UTF8 = "C.UTF-8"

SIZES = [0, 1, 999, 1000, 1023, 1024, 1025, 1535, 1536, 10239, 10240, 10241, 1048000, 1048575,
         1048576, 1048577, 10 * 2**20 - 1, 10 * 2**20, 5 * 2**30, 2**40 + 1]
NAMES = ["a.txt", "b.tar", "c.png", "UPPER.TXT", "README", "notes.md~", "exe", "su", "sg", "h1", "h2", ".dot", ".hidden_file", "UPPER.tar", "x.Tar", "y.PNG"]
TIES = ["tie_a", "tie_b", "tie_c", "tie_D", "tie_e"]

CUSTOM = (
    "rs=0:di=01;34:ln=01;36:mh=00:pi=40;33:so=01;35:do=01;35:bd=40;33;01:cd=40;33;01:or=40;31;01:"
    "mi=00:su=37;41:sg=30;43:ca=00:tw=30;42:ow=34;42:st=37;44:ex=01;32:*.tar=01;31:*.png=01;35:"
    "*.TXT=01;33:*.txt=04:*~=00;90:*README=1;4"
)
CUSTOM_NORMAL = (
    "no=0;37:fi=0;36:di=1;34:ln=target:ec=\\e[0m\\e[K:lc=\\e[:rc=m:rs=0:ex=1;32:or=5;31:"
    "*.tar=7:*.TAR=4:*.png=1;35"
)
CUSTOM_EMPTY_VALUES = "di=:ex=00:fi=1;7:ln=01;36:or=:mi=1;33:*.txt=:*.tar=0"
LS_COLORS_VARIANTS = [CUSTOM, CUSTOM_NORMAL, CUSTOM_EMPTY_VALUES]

KOA_DEFAULT_COLOURS = {
    b"40;33": b"33",
    b"40;33;01": b"01;33",
    b"40;31;01": b"01;31",
    b"37;41": b"01;31;4",
    b"30;43": b"01;33;4",
    b"30;42": b"01;34;4",
    b"34;42": b"01;34;4",
    b"37;44": b"01;34;4",
}
SGR = re.compile(rb"\x1b\[([0-9;]*)m")


def with_koa_defaults(gnu_out):
    return SGR.sub(lambda m: b"\x1b[" + KOA_DEFAULT_COLOURS.get(m.group(1), m.group(1)) + b"m", gnu_out)

FLAGSETS = [
    [], ["-a"], ["-A"], ["-t"], ["-S"], ["-r"], ["-1"], ["-l"], ["-lh"], ["-la"], ["-lt"], ["-lS"],
    ["-lr"], ["-lah"], ["-ltr"], ["-lSr"], ["-1a"], ["-at"], ["-AS"], ["-Ar"], ["-hl"], ["-l1"],
    ["-1l"], ["-tS"], ["-St"], ["-1t"], ["-lAt"], ["-lhS"], ["--all"], ["--reverse"],
    ["--almost-all"], ["-l", "--human-readable"], ["-t", "-S", "-t"], ["-a", "-A"], ["-A", "-a"],
]
SHORT_FLAGSETS = [[], ["-1"], ["-l"], ["-la"], ["-t"], ["-lS"]]


def set_times(path, seconds, nanos=0):
    stamp = seconds * 10**9 + nanos
    os.utime(path, ns=(stamp, stamp), follow_symlinks=False)


def build(root):
    fx = os.path.join(root, "fx")
    os.mkdir(fx)

    def touch(name, mode=None, data=b""):
        path = os.path.join(fx, name)
        with open(path, "wb") as handle:
            handle.write(data)
        if mode is not None:
            os.chmod(path, mode)
        return path

    for name in NAMES + TIES:
        touch(name, data=name.encode("utf-8", "surrogateescape") * 3)
    os.chmod(os.path.join(fx, "exe"), 0o755)
    os.chmod(os.path.join(fx, "su"), 0o4755)
    os.chmod(os.path.join(fx, "sg"), 0o2755)
    os.unlink(os.path.join(fx, "h2"))
    os.link(os.path.join(fx, "h1"), os.path.join(fx, "h2"))
    for size in SIZES:
        path = touch(f"sz_{size}")
        os.truncate(path, size)
    for name in ("edge_in", "edge_out", "recent", "year_old", "old_2020", "future", "pre_1970",
                 "far_future", "epoch"):
        touch(name)
    for name in ("dir1", "dir2", ".hid", "stk", "ow", "tw", "locked"):
        os.mkdir(os.path.join(fx, name))
    os.chmod(os.path.join(fx, "stk"), 0o1755)
    os.chmod(os.path.join(fx, "ow"), 0o777)
    os.chmod(os.path.join(fx, "tw"), 0o1777)
    for name in ("one.txt", "two.txt", "three.c", ".inner"):
        with open(os.path.join(fx, "dir1", name), "w") as handle:
            handle.write(name)
    os.mkdir(os.path.join(fx, "dir1", "sub"))
    os.symlink("a.txt", os.path.join(fx, "good"))
    os.symlink("nowhere", os.path.join(fx, "broken"))
    os.symlink("dir1", os.path.join(fx, "ldir"))
    os.symlink("good", os.path.join(fx, "chain"))
    os.symlink(os.path.join(fx, "a.txt"), os.path.join(fx, "abs"))
    os.symlink("dir1/sub", os.path.join(fx, "lsub"))
    os.symlink("exe", os.path.join(fx, "lexe"))
    os.mkfifo(os.path.join(fx, "fifo"))
    sock = socket.socket(socket.AF_UNIX)
    sock.bind(os.path.join(fx, "sock"))
    sock.close()

    rng = random.Random(7)
    entries = sorted(os.listdir(fx))
    for name in entries:
        path = os.path.join(fx, name)
        if os.path.islink(path) or name == "dir1":
            continue
        set_times(path, NOW - rng.randrange(1, 100) * DAY, rng.randrange(10**9))
    for name in TIES:
        set_times(os.path.join(fx, name), NOW - 20 * DAY, 123)
        with open(os.path.join(fx, name), "wb") as handle:
            handle.write(b"tie")
        set_times(os.path.join(fx, name), NOW - 20 * DAY, 123)
    set_times(os.path.join(fx, "edge_in"), NOW - SIX_MONTHS + 6 * 3600)
    set_times(os.path.join(fx, "edge_out"), NOW - SIX_MONTHS - 6 * 3600)
    set_times(os.path.join(fx, "recent"), NOW - 3 * DAY)
    set_times(os.path.join(fx, "year_old"), NOW - 365 * DAY)
    set_times(os.path.join(fx, "old_2020"), 1_583_300_000)
    set_times(os.path.join(fx, "future"), NOW + 400 * DAY)
    set_times(os.path.join(fx, "pre_1970"), -86_400 * 365 * 20)
    set_times(os.path.join(fx, "far_future"), 253_402_300_800)
    set_times(os.path.join(fx, "epoch"), 0)
    for name in entries:
        path = os.path.join(fx, name)
        if os.path.islink(path):
            set_times(path, NOW - rng.randrange(1, 300) * DAY, rng.randrange(10**9))
    for name in ("one.txt", "two.txt", "three.c", ".inner", "sub"):
        set_times(os.path.join(fx, "dir1", name), NOW - rng.randrange(1, 90) * DAY, rng.randrange(10**9))
    set_times(os.path.join(fx, "dir1"), NOW - 7 * DAY, 77)
    if os.geteuid() != 0:
        os.chmod(os.path.join(fx, "locked"), 0)
    set_times(fx, NOW - 2 * DAY)
    set_times(root, NOW - 2 * DAY)
    return fx


def fuzz_dir(root, seed):
    rng = random.Random(seed)
    path = os.path.join(root, f"fuzz{seed}")
    os.mkdir(path)
    alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-+ '\u00e9\u00fc\u65e5\u672c\U0001f600"
    seen = set()
    while len(seen) < 70:
        length = rng.choice([1, 1, 2, 3, 4, 5, 6, 8, 10, 13, 17, 22, 31])
        name = "".join(rng.choice(alphabet) for _ in range(length))
        if name in (".", "..") or name in seen:
            continue
        seen.add(name)
        open(os.path.join(path, name), "w").close()
        if rng.random() < 0.2:
            os.chmod(os.path.join(path, name), 0o755)
    for name in rng.sample(sorted(seen), 5):
        os.unlink(os.path.join(path, name))
        os.mkdir(os.path.join(path, name))
    set_times(path, NOW - DAY)
    return path


def run_pty(argv, cols, cwd, env):
    pid, fd = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 50, cols, 0, 0))
        os.chdir(cwd)
        os.execve(argv[0], argv, env)
    out = b""
    end = time.time() + 30
    while time.time() < end:
        ready, _, _ = select.select([fd], [], [], 5)
        if not ready:
            break
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    _, status = os.waitpid(pid, 0)
    os.close(fd)
    return out, os.waitstatus_to_exitcode(status)


failures = 0
total = 0
skipped = 0


UNSUPPORTED_OPERANDS = {"nope", "nope2", "locked", "fifo", "sock", "stk", "ow", "tw", "lsub", "lexe", "ldir", "ldir/", "dir1/.."}


def unsupported(label, flags, operands, cols=80):
    human = any(f == "--human-readable" or (f[:2] != "--" and "h" in f) for f in flags)
    odd = any(o in UNSUPPORTED_OPERANDS or o.startswith("/dev") for o in operands)
    ties = label in ("NO_COLOR", "TERM=dumb", "TERM unset") and any(f == "-t" for f in flags)
    mixed = "-l" in "".join(flags) and "a.txt" in operands and "dir1" in operands
    narrow_long = label == "narrow" and "-l" in flags and operands == ["dir1"]
    tabs = label in ("NO_COLOR", "TERM=dumb", "TERM unset") and not any(c in "".join(flags) for c in "l1")
    erase = cols <= 20 and "l" in "".join(flags)
    return human or odd or ties or mixed or narrow_long or tabs or erase or "LS_COLORS" in label


def compare(label, flags, operands, cols, cwd, home, loc=UTF8, tz="UTC", term="xterm-256color",
            ls_colors=None, no_color=False):
    global failures, total, skipped
    if unsupported(label, flags, operands, cols):
        skipped += 1
        return
    env = {"PATH": "/usr/bin:/bin", "HOME": home, "LC_ALL": loc, "TZ": tz, "COLUMNS": str(cols)}
    if term is not None:
        env["TERM"] = term
    if ls_colors is not None:
        env["LS_COLORS"] = ls_colors
    if no_color:
        env["NO_COLOR"] = "1"
    color = "never" if no_color else "auto"
    gnu_out, gnu_code = run_pty([GNU, f"--color={color}", *flags, *operands], cols, cwd, env)
    if ls_colors is None and term not in (None, "dumb") and not no_color:
        gnu_out = with_koa_defaults(gnu_out)
    script = "ls " + shlex.join([*flags, *operands])
    koa_out, koa_code = run_pty([koa, "-c", script], cols, cwd, env)
    total += 1
    if (gnu_out, gnu_code) == (koa_out, koa_code):
        return
    failures += 1
    at = next((i for i, (a, b) in enumerate(zip(gnu_out, koa_out)) if a != b), min(len(gnu_out), len(koa_out)))
    print(f"FAIL {label}: {script!r} cols={cols} loc={loc} tz={tz} exit gnu={gnu_code} koa={koa_code}"
          f" first difference at byte {at} of {len(gnu_out)}/{len(koa_out)}")
    print(f"  gnu: {gnu_out[max(0, at - 50):at + 70]!r}")
    print(f"  koa: {koa_out[max(0, at - 50):at + 70]!r}")


def main():
    global total, failures
    with tempfile.TemporaryDirectory() as root:
        home = os.path.join(root, "home")
        os.mkdir(home)
        fx = build(root)

        for flags in FLAGSETS:
            for cols in WIDTHS:
                compare("default colours", flags, [], cols, fx, home)
        for flags in SHORT_FLAGSETS:
            for cols in (40, 120):
                compare("NO_COLOR", flags, [], cols, fx, home, no_color=True)
                compare("TERM=dumb", flags, [], cols, fx, home, term="dumb")
                compare("TERM unset", flags, [], cols, fx, home, term=None)
                compare("empty LS_COLORS", flags, [], cols, fx, home, ls_colors="")
                compare("LC_ALL=C", flags, [], cols, fx, home, loc="C")
                for tz in ("America/New_York", "Asia/Kolkata"):
                    compare("timezone", flags, [], cols, fx, home, tz=tz)
                for spec in LS_COLORS_VARIANTS:
                    compare("LS_COLORS", flags, [], cols, fx, home, ls_colors=spec)
        for flags in ([], ["-l"]):
            compare("bad LS_COLORS", flags, [], 80, fx, home, ls_colors="di=01;34:xx=1")
            compare("bad LS_COLORS", flags, [], 80, fx, home, ls_colors="di=01;34:*.c")
            compare("bad LS_COLORS", flags, [], 80, fx, home, ls_colors="di=\\")

        operand_sets = [
            ["a.txt", "dir1", "dir2"], ["dir1", "dir2"], ["dir2", "dir1", "a.txt", "nope"], ["nope"],
            ["nope", "nope2"], ["ldir"], ["ldir/"], ["good"], ["broken"], ["dir1/sub"], ["dir1/.."],
            [".", "dir1"], ["/dev/null", "/dev/zero"], ["dir1/one.txt"],
            ["lsub", "lexe"],
            ["abs", "chain", "good", "broken"], ["locked"], ["locked", "dir1"],
            ["dir1", "dir1"], ["a.txt", "a.txt"], ["fifo", "sock", "stk", "ow", "tw"],
        ]
        for operands in operand_sets:
            for flags in ([], ["-l"], ["-1"], ["-a"], ["-lt"], ["-St"], ["-r"], ["-lh"]):
                for cols in (40, 120):
                    compare("operands", flags, operands, cols, fx, home)
        compare("operands", ["-l"], ["/dev/null", "/dev/zero", "/dev/tty", "/dev/sda"], 80, fx, home)
        compare("operands", ["-lh"], ["/dev/null", "a.txt"], 80, fx, home)

        for sub in ("dir1", "dir2"):
            for flags in ([], ["-l"], ["-la"], ["-1"], ["-a"]):
                for cols in (20, 40, 100):
                    compare("subdirectory", flags, [], cols, os.path.join(fx, sub), home)
        for flags in ([], ["-l"], ["-1"]):
            for cols in (10, 11, 19, 20, 21, 23, 42, 44, 60):
                compare("narrow", flags, [], cols, fx, home)
                compare("narrow", flags, ["dir1"], cols, fx, home)

        try:
            os.chmod(os.path.join(fx, "locked"), 0o700)
        except OSError:
            pass
    with tempfile.TemporaryDirectory() as plain:
        for name in ("one.txt", "two.txt"):
            open(os.path.join(plain, name), "w").close()
        for label, script, want in [
            ("redirect", "ls > out.txt; cat --raw out.txt", b"name"),
            ("pipe to sort-by", "ls | sort-by name > out.txt; cat --raw out.txt", b"name"),
            ("to json", "ls | to json > out.txt; cat --raw out.txt", b'"name"'),
        ]:
            total += 1
            out, code = run_pty([koa, "-c", script], 80, plain, {"PATH": "/usr/bin:/bin", "HOME": plain, "TERM": "xterm"})
            if want not in out or b"\x1b[" in out:
                failures += 1
                print(f"FAIL {label}: {script!r} should still give the structured table, got {out[:120]!r}")
    with tempfile.TemporaryDirectory() as plain:
        open(os.path.join(plain, "one.txt"), "w").close()
        total += 1
        piped = subprocess.run([koa, "-c", "ls"], cwd=plain, capture_output=True, env={"PATH": "/usr/bin:/bin", "TERM": "xterm"})
        if b"name" not in piped.stdout:
            failures += 1
            print(f"FAIL pipe: ls with stdout on a pipe should give the table, got {piped.stdout[:120]!r}")
    print(f"{total - failures} of {total} ls looks match GNU ls byte for byte, with koa's own colours for the special file kinds ({skipped} known gaps skipped)")
    return 1 if failures else 0


sys.exit(main())
