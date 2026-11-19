#!/usr/bin/env python3
import os
import shlex
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import fcntl
import select
import termios

if shutil.which("eza") is None:
    print("eza is not installed, skipping the eza look check")
    sys.exit(0)

koa = os.path.abspath(sys.argv[1])
ALIAS = ["eza", "-lh", "--group-directories-first", "--icons=auto"]
LINES = ["eza", "-1", "--group-directories-first", "--icons=auto"]
DAY = 86_400


def run(argv, cwd, env, cols=120):
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, cols, 0, 0))
    proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=slave, stdout=slave, stderr=slave, close_fds=True)
    os.close(slave)
    out = b""
    while True:
        try:
            chunk = os.read(master, 1 << 16)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
    proc.wait()
    os.close(master)
    return out


def repl(cwd, env, command, cols=120):
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, cols, 0, 0))
    proc = subprocess.Popen([koa], cwd=cwd, env=env, stdin=slave, stdout=slave, stderr=slave, close_fds=True)
    os.close(slave)
    out = b""

    def until(marker, start):
        nonlocal out
        deadline = time.time() + 20
        while out.find(marker, start) < 0 and time.time() < deadline:
            if select.select([master], [], [], 0.2)[0]:
                try:
                    out += os.read(master, 1 << 16)
                except OSError:
                    break

    until(b"koa> ", 0)
    mark = len(out)
    os.write(master, command.encode() + b"\r")
    until(b"Permissions", mark)
    start = out.index(b"Permissions", mark)
    until(b"koa> ", start)
    os.write(master, b"exit\r")
    proc.wait(timeout=20)
    os.close(master)
    if out[start - 4:start] == b"\x1b[4m":
        start -= 4
    return out[start:out.index(b"\x1b[1G\x1b[K", start)]


def utime(path, when):
    os.utime(path, (when, when), follow_symlinks=False)


def build(root):
    now = int(time.time())
    year = time.localtime(now).tm_year

    def make(name, size=0, age_days=0, mode=None, data=None):
        path = os.path.join(root, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as fh:
            if data is not None:
                fh.write(data)
            elif size:
                fh.truncate(size)
        if mode is not None:
            os.chmod(path, mode)
        utime(path, now - age_days * DAY - 3600)
        return path

    def folder(name, age_days=0, mode=None):
        path = os.path.join(root, name)
        os.makedirs(path, exist_ok=True)
        if mode is not None:
            os.chmod(path, mode)
        utime(path, now - age_days * DAY - 7200)

    for name in ["zdir", "dir one", ".hiddendir", "ünï dir", "node_modules", "src", ".git", "emptydir", "sticky"]:
        folder(name, age_days=3)
    make("zdir/inner.txt")
    make("dir one/in.txt")
    make(".git/HEAD")
    make("node_modules/x.js")
    make("src/main.rs")
    make("ünï dir/f")
    os.chmod(os.path.join(root, "sticky"), 0o1777)
    for name, args in {
        "a.rs": {"data": b"fn main() {}\n"},
        "big.bin": {"size": 24_000},
        "mid.dat": {"size": 1_500},
        "run.sh": {"data": b"#!/bin/sh\n", "mode": 0o755},
        "setuid": {"mode": 0o4755},
        "setgid": {"mode": 0o2755},
        "noperm": {"mode": 0o000},
        "worldw": {"mode": 0o666},
        "stickyfile": {"mode": 0o1644},
        "setuid-noexec": {"mode": 0o4644},
        ".hidden": {},
        ".bashrc": {},
        ".gitignore": {},
        "file with space.txt": {},
        "ünï cödé.txt": {},
        "it's.txt": {},
        'dq"uote.txt': {},
        "new\nline.txt": {},
        "esc\x1b[31m.txt": {},
        "tab\there": {},
        "日本語.txt": {},
        "f2": {},
        "f10": {},
        "f9": {},
        "Zebra": {},
        "apple": {},
        "Mango": {},
        "Makefile": {},
        "README.md": {},
        "readme.txt": {},
        "Cargo.toml": {},
        "package.json": {},
        "Dockerfile": {},
        "pic.png": {},
        "clip.mp4": {},
        "song.mp3": {},
        "lossless.flac": {},
        "doc.pdf": {},
        "arch.tar.gz": {},
        "stale.bak": {},
        "key.pem": {},
        "id_rsa": {},
        "cc.c": {},
        "x.ts": {},
        "x.js": {},
        "lone.js": {},
        "style.scss": {},
        "style.css": {},
        "lib.o": {},
        "tex.tex": {},
        "tex.aux": {},
        "file~": {},
        "#autosave#": {},
        "noext": {},
        "tarball.TAR": {},
        "UPPER.RS": {},
    }.items():
        make(name, **args)
    for size in [0, 1, 999, 1000, 1049, 1050, 1051, 1249, 1250, 1251, 1999, 2050, 9949, 9950, 9951, 9999, 10000,
                 10499, 10500, 99_949, 99_950, 99_999, 100_000, 999_499, 999_500, 999_999, 1_000_000,
                 1_450_000, 123_456_789, 5_000_000_000, 2_000_000_000_000]:
        make(f"sizes/s{size:013d}", size=size)
    for label, when in {
        "old.txt": time.mktime((2024, 3, 5, 10, 11, 0, 0, 0, -1)),
        "lastyear.txt": time.mktime((year - 1, 1, 2, 3, 4, 5, 0, 0, -1)),
        "y2000.txt": time.mktime((2000, 12, 31, 23, 59, 0, 0, 0, -1)),
        "jan1.txt": time.mktime((year, 1, 1, 0, 0, 0, 0, 0, -1)),
        "epoch.txt": 0,
        "five.txt": now - 150 * DAY,
        "seven.txt": now - 210 * DAY,
        "tomorrow.txt": now + DAY,
        "nextyear.txt": now + 400 * DAY,
    }.items():
        make(label)
        utime(os.path.join(root, label), int(when))
    make("negative.txt")
    subprocess.run(["touch", "-d", "@-1000", os.path.join(root, "negative.txt")], check=True)
    for n, age in enumerate([1, 2, 3, 40, 41, 100, 200, 300, 500, 900]):
        make(f"ages/f{n}", age_days=age)
    os.symlink("a.rs", os.path.join(root, "link.rs"))
    os.symlink("dir one", os.path.join(root, "dlink"))
    os.symlink("dir one/", os.path.join(root, "tslink"))
    os.symlink("nowhere", os.path.join(root, "broken"))
    os.symlink("no where/x y", os.path.join(root, "broken space"))
    os.symlink(os.path.join(root, "run.sh"), os.path.join(root, "abslink"))
    os.symlink("/etc/hosts", os.path.join(root, "hostslink"))
    os.symlink("/usr/bin/", os.path.join(root, "usrbin"))
    os.symlink("../a.rs", os.path.join(root, "zdir/up"))
    os.symlink("zdir/inner.txt", os.path.join(root, "deep"))
    os.symlink("file with space.txt", os.path.join(root, "link to space"))
    os.symlink("it's.txt", os.path.join(root, "quote link"))
    os.symlink("link.rs", os.path.join(root, "chain"))
    os.symlink("chain", os.path.join(root, "chain2"))
    os.symlink("loop2", os.path.join(root, "loop1"))
    os.symlink("loop1", os.path.join(root, "loop2"))
    os.symlink("src", os.path.join(root, "srclink"))
    os.symlink("Makefile", os.path.join(root, "Makelink"))
    os.symlink("x.ts", os.path.join(root, "x.link"))
    os.symlink("nowhere\x1b", os.path.join(root, "broken-ctl"))
    os.mkfifo(os.path.join(root, "fifo"))
    sock = socket.socket(socket.AF_UNIX)
    sock.bind(os.path.join(root, "sock"))
    sock.close()
    try:
        os.setxattr(os.path.join(root, "a.rs"), "user.koa", b"1")
        os.setxattr(os.path.join(root, "zdir"), "user.koa", b"1")
        os.setxattr(os.path.join(root, "dlink"), "user.koa", b"1", follow_symlinks=False)
    except OSError:
        pass
    for name in ["a1", "a01", "a001", "a10", "a2", "a02", "1", "01", "001", "10", "2", "b1", "B2", "b 3", "b-1",
                 "a-1", "a_1", "x1y", "x01y", "x10y", "file 9", "file10", "file 11", "v1.2", "v1.10", "v1.02",
                 "école", "Zoë", "ÀB", "ab", "Ab2", "ab10", "a b", "abc", "i1", "i 1x", "z00", "z0", "z000a", "z0a"]:
        make(f"natural/{name}")
    os.makedirs(os.path.join(root, "narrow"))
    for name in ["a", "bb", "ccc"]:
        make(f"narrow/{name}", size=1234)
    for n in range(1, 31):
        make(f"many/file{n:03d}.txt", size=n * 137)
    for n in range(5000):
        make(f"bulk/item{n * 7919 % 5000:04d}.{['txt', 'rs', 'md', 'png', 'o'][n % 5]}", size=n % 3000)
    for n in range(40):
        os.makedirs(os.path.join(root, f"bulk/dir{n:02d}"))


HOME = tempfile.mkdtemp(prefix="koa-eza-home-")


def base_env(**extra):
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": HOME,
        "TERM": "xterm-256color",
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TZ": "Asia/Kolkata",
    }
    env.update(extra)
    return env


LS_COLORS_RICH = (
    "rs=0:di=01;34:ln=01;36:mh=00:pi=40;33:so=01;35:do=01;35:bd=40;33;01:cd=40;33;01:or=40;31;01:mi=00:su=37;41:"
    "sg=30;43:ca=00:tw=30;42:ow=34;42:st=37;44:ex=01;32:*.tar=01;31:*.gz=01;31:*.png=01;35:*.mp3=00;36:"
    "*.rs=38;5;208:*~=2;37:README*=4;33:*.[ch]=32:*.tar.gz=1;4;31:Makefile=1;33;41:a.rs=38;2;10;200;30:*.txt=48;5;22"
)

CASES = []


def case(label, koa_args, eza_args=None, env=None, cols=120, cwd=".", lines=False):
    CASES.append((label, koa_args, eza_args if eza_args is not None else koa_args, env or {}, cols, cwd, lines))


case("default", [])
case("hidden", ["-a"])
case("hidden long flags", ["-lha"])
case("almost-all", ["-A"], ["-a"])
case("reverse", ["-r"])
case("reverse long", ["--reverse"], ["--reverse"])
case("newest first", ["-t"], ["--sort=age"], cwd="ages")
case("largest first", ["-S"], ["--sort=size", "--reverse"], cwd="sizes")
case("largest first reversed", ["-S", "-r"], ["--sort=size"], cwd="sizes")
case("sizes", [], cwd="sizes")
case("one per line", ["-1"], lines=True)
case("one per line hidden", ["-1", "-a"], ["-a"], lines=True)
case("long beats one", ["-l", "-1"], [])
case("one per line dirs and files", ["-1", "zdir", "a.rs", "dir one"], ["zdir", "a.rs", "dir one"], lines=True)
case("one per line newest", ["-1", "-t"], ["--sort=age"], cwd="ages", lines=True)
case("one per line reverse", ["-1", "-r"], ["-r"], lines=True)
case("human flag", ["-h"])
case("subdir", ["zdir"])
case("space dir", ["dir one"])
case("empty dir", ["emptydir"])
case("two dirs", ["zdir", "dir one"])
case("dir and files", ["zdir", "a.rs", "dir one", "link.rs"])
case("files only", ["a.rs", "Makefile", "run.sh", "dlink"])
case("path operands", ["zdir/inner.txt", "./a.rs", "../work/Makefile", "/etc/hosts", "zdir/up"])
case("slash dir", ["zdir/"])
case("link to dir operand", ["dlink"])
case("trailing slash link operand", ["tslink"])
case("broken operand", ["broken"])
case("root", ["/"])
case("dev", ["/dev"])
case("dev nodes", ["/dev/null", "/dev/zero", "/dev/tty"])
case("narrow dir", ["narrow"], cols=40)
case("narrow default", [], cols=40)
case("many", ["many"])
case("bulk directory", ["bulk"])
case("bulk directory reversed", ["-r", "bulk"])
case("bulk one per line", ["-1", "bulk"], ["bulk"], lines=True)
case("natural order", ["natural"])
case("natural order reversed", ["-r", "natural"])
case("no color", [], env={"NO_COLOR": "1"})
case("no color empty value", [], env={"NO_COLOR": ""})
case("no color one per line", ["-1"], ["-1"], env={"NO_COLOR": "1"}, lines=True)
case("no color hidden", ["-a"], env={"NO_COLOR": "1"})
case("ls colors rich", ["-a"], env={"LS_COLORS": LS_COLORS_RICH})
case("ls colors rich one per line", ["-1"], ["-1"], env={"LS_COLORS": LS_COLORS_RICH}, lines=True)
case("ls colors bad", [], env={"LS_COLORS": "di=1;38;5;999:ex=zzz:*.rs=38;2;1;2:*.md=:=1:foo=1=2:ln=:*.[=31"})
case("eza colors", ["-a"], env={"EZA_COLORS": "ur=31:uw=32:ux=33:ue=34:gr=35:gw=36:gx=37:tr=91:tw=92:tx=93:da=1;35:sn=4:sb=7:uu=44:un=45:hd=1;4;31:xx=38;5;99:lp=33:di=4:ex=48;5;17:lc=1:*.rs=1;4:mp=5"})
case("eza colors sizes", ["-a"], cwd="sizes", env={"EZA_COLORS": "nb=31:nk=32:nm=33:ng=34:nt=35:ub=36:uk=37:um=91:ug=92:ut=93"})
case("eza colors reset", ["-a"], env={"EZA_COLORS": "reset"})
case("eza colors reset more", ["-a"], env={"EZA_COLORS": "reset:*.rs=31:di=32"})
case("eza colors beat ls colors", [], env={"LS_COLORS": "di=31:*.rs=32", "EZA_COLORS": "di=33"})
case("exa colors fallback", [], env={"EXA_COLORS": "di=35"})
case("icon spacing", [], env={"EZA_ICON_SPACING": "3"})
case("icon spacing zero", ["-1"], ["-1"], env={"EZA_ICON_SPACING": "0"}, lines=True)


def dircolors():
    out = subprocess.run(["dircolors", "-b"], capture_output=True, text=True)
    if out.returncode != 0:
        return None
    for part in out.stdout.split(";"):
        part = part.strip()
        if part.startswith("LS_COLORS='"):
            return part[len("LS_COLORS='"):-1]
    return None


stock = dircolors()
if stock:
    case("stock dircolors", ["-a"], env={"LS_COLORS": stock})
    case("stock dircolors one per line", ["-1"], ["-1"], env={"LS_COLORS": stock}, lines=True)


bad = []
with tempfile.TemporaryDirectory(prefix="koa-eza-look-") as scratch:
    root = os.path.join(scratch, "work")
    os.mkdir(root)
    build(root)
    for label, koa_args, eza_args, env, cols, cwd, lines in CASES:
        where = os.path.join(root, cwd)
        want_cmd = (LINES if lines else ALIAS) + eza_args
        koa_cmd = [koa, "-c", " ".join(["ls", *map(shlex.quote, koa_args)])]
        for attempt in range(2):
            run_env = base_env(**env)
            want = run(want_cmd, where, run_env, cols)
            got = run(koa_cmd, where, run_env, cols)
            if got == want:
                break
        else:
            bad.append((label, want_cmd, koa_cmd, want, got))

    run_env = base_env()
    want = run(ALIAS, os.path.join(root, "narrow"), run_env)
    got = run([koa, "-c", "cd narrow; ls"], root, run_env)
    if got != want:
        bad.append(("session cd", ALIAS, [koa, "-c", "cd narrow; ls"], want, got))
    for label, env in [("repl", {}), ("repl no color", {"NO_COLOR": "1"})]:
        where = os.path.join(root, "narrow")
        run_env = base_env(**env)
        want = run(ALIAS, where, run_env)
        got = repl(where, run_env, "ls")
        if got != want:
            bad.append((label, ALIAS, [koa, "(repl)", "ls"], want, got))

    piped = subprocess.run([koa, "-c", "ls"], cwd=root, capture_output=True, text=True, env=base_env())
    if "Permissions" in piped.stdout or not piped.stdout.startswith("ext"):
        bad.append(("piped stays a table", [], [], b"name ...", piped.stdout.encode()))
    redirected = os.path.join(scratch, "out.txt")
    subprocess.run([koa, "-c", f"ls > {shlex.quote(redirected)}"], cwd=root, env=base_env(), check=True)
    with open(redirected) as fh:
        if not fh.readline().startswith("{"):
            bad.append(("redirect stays records", [], [], b"{...", b"other"))

for label, want_cmd, koa_cmd, want, got in bad:
    print(f"MISMATCH {label}: {shlex.join(want_cmd)} vs {shlex.join(koa_cmd)}")
    want_lines = want.split(b"\r\n")
    got_lines = got.split(b"\r\n")
    shown = 0
    for n in range(max(len(want_lines), len(got_lines))):
        left = want_lines[n] if n < len(want_lines) else b"<missing>"
        right = got_lines[n] if n < len(got_lines) else b"<missing>"
        if left != right:
            print(f"  line {n}\n    eza {left!r}\n    koa {right!r}")
            shown += 1
            if shown == 3:
                break
total = len(CASES) + 3
print(f"{total - len(bad)} of {total} eza look cases match byte for byte")
sys.exit(1 if bad else 0)
