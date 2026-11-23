#!/usr/bin/env python3
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
bad = []
checked = 0

SHOW = "printf '[%s]' "
ENV = {"PATH": os.environ["PATH"], "LC_ALL": "C.UTF-8", "HOME": "/nonexistent"}

ANSI_C = [
    r"\a", r"\b", r"\e", r"\E", r"\f", r"\n", r"\r", r"\t", r"\v", r"\\", r"\'", r'\"', r"\?",
    r"\1", r"\12", r"\101", r"\1012", r"\0101", r"\x41", r"\x4", r"\x4g", r"\x414", r"\x", r"\xZZ",
    r"\u00e9", r"\u20ac", r"\u41", r"\u", r"\uZZ", r"\U0001F600", r"\U41", r"\U", r"\q", r"\8", r"\ ",
    r"\cA", r"\cz", r"\c?", r"\c[", r"\c\\",
    "plain", "a b", "tab\tinside", "\u00e9t\u00e9 \u65e5\u672c", "$HOME", "$(echo no)", "*", "{a,b}", "~", "#",
    "a;b", "a|b", "a&b", "a>b", "a<b", "(x)", r"\n\n", r"a\nb\tc", r"\x41\102\u0043", r"\e[1m\e[0m",
]
ANSI_SCRIPTS = (
    [f"{SHOW}$'{body}'" for body in ANSI_C]
    + [
        f"{SHOW}a$'b'c", f"{SHOW}''$'x'", f"{SHOW}$'x'$'y'", f"{SHOW}$'a' $'b'", f"{SHOW}\"$'x'\"",
        f"{SHOW}\"a $'b' c\"", f"{SHOW}'$'\"'x'\"", f"{SHOW}$'a'\"b\"'c'", f"{SHOW}$'x'y$HOME", f"{SHOW}$'a\\nb' c",
        f"v=$'a\\tb'; {SHOW}\"$v\"", f"{SHOW}x=$'y'", f"$'echo' hi", f"$'e'cho hi",
        "echo hi > $'f\\tg'", "echo hi > $'a b'$'c'", f"{SHOW}$'a'*", f"{SHOW}$'*'", f"{SHOW}$'x",
        f"{SHOW}$'a' && echo ok", f"echo -n $'a\\tb\\n\\x41\\102\\u0043'", f"echo $'it\\'s'",
        f"{SHOW}$'\\'", f"{SHOW}$'\\\\'$'\\''", f"{SHOW}$", f"{SHOW}a$", f"{SHOW}$'a'$",
    ]
)

VARIABLES = {"set": "hello", "empty": "", "uni": "\u00e9t\u00e9\u65e5", "sp": "a b", "star": "*"}
WORDS = ["d", "", "dd ee", "x-y", "a:b", "a}b", "*", "~", "#", "-", "+", "=", ":-", "%"]
EXPANSIONS = []
for var in ["set", "empty", "unset", "uni", "sp", "star", "?"]:
    EXPANSIONS.append(f"${{#{var}}}")
    for word in WORDS:
        for op in (":-", "-", ":+", "+"):
            EXPANSIONS.append(f"${{{var}{op}{word}}}")
EXPANSIONS += [
    "${set:-}", "${set-}", "${empty:-}", "${unset:-}", "${unset:+}", "${set:+}", "${#}", "${#unset}",
    "${set:-$empty}", "${unset:-$set}", "${unset:-${set}}", "${unset:-`echo x`}", "${unset:-$(echo x)}",
    "${unset:=x}", "${unset:?}", "${set%l*}", "${set#h}", "${set/l/L}", "${set:1}", "${set:1:2}", "${set^^}",
    "${1:-d}", "${1}", "${HOME:-h}",
    "${}", "${ set}", "${set }", "${#set:-d}", "${set:-d", "${:-d}", "${-}", "${@:-d}", "${*:-d}",
    "${set::-d}", "${set:--d}", "${set:-d}}", "${set:-\\}}", "${set:-\"q\"}", "${unset:-'q'}", "${unset:-\\ }",
]
def splits_fields(expansion):
    return " " in expansion or expansion.startswith(("${sp", "${star", "${#sp", "${#star"))


PARAM_SCRIPTS = []
NATIVE_WORDS = []
for expansion in EXPANSIONS:
    PARAM_SCRIPTS.append(f"{SHOW}\"{expansion}\"")
    NATIVE_WORDS.append(f"\"{expansion}\"")
    if not splits_fields(expansion):
        PARAM_SCRIPTS.append(f"{SHOW}{expansion}")
        PARAM_SCRIPTS.append(f"{SHOW}x{expansion}y")
        NATIVE_WORDS.append(expansion)
PARAM_SCRIPTS += [
    "echo ${#set} ${unset:-fallback} ${empty:-empty_fb} ${empty-set_empty} ${set:+present} ${unset:+nope}",
    "FOO=hello; EMPTY=''; echo ${#FOO} ${UNSET:-fallback} ${EMPTY:-empty_fb} ${EMPTY-set_empty} ${FOO:+present} ${UNSET:+nope}",
    f"FOO=a; {SHOW}${{FOO:-b}} ${{BAR:-c}}",
    f"X=${{unset:-assigned}}; {SHOW}\"$X\"",
    f"{SHOW}${{unset:-a}}${{unset:-b}}",
    f"{SHOW}${{#set}}${{#empty}}",
    f"echo ${{unset:-out}} > ${{unset:-redir}}",
    f"echo ${{unset:-hi}} | cat",
    f"false; echo ${{?}} ${{#?}} ${{?:-d}}",
    f"true && echo ${{unset:-and}} || echo no",
    f"{SHOW}\"${{unset:-a   b}}\"",
    f"{SHOW}\"${{sp:+x y}}\"",
    f"{SHOW}${{unset:-$HOME}}",
]

PIPE_BOTH = [
    "sh -c 'echo out; echo err >&2' |& cat",
    "sh -c 'echo out; echo err >&2' | cat",
    "sh -c 'echo out; echo err >&2' |& sh -c 'grep err'",
    "sh -c 'echo out; echo err >&2' |& sh -c 'grep -c .'",
    "sh -c 'echo err >&2' |& cat",
    "sh -c 'echo err >&2' |& cat |& cat",
    "sh -c 'echo a; echo b >&2' |& sh -c 'echo first; cat; echo last >&2' |& cat",
    "sh -c 'echo a; echo b >&2' | sh -c 'echo first; cat; echo last >&2' |& cat",
    "sh -c 'echo a; echo b >&2' |& sh -c 'cat >&2' 2>/dev/null",
    "sh -c 'echo err >&2' 2>/dev/null |& cat",
    "sh -c 'echo err >&2' 2>&1 |& cat",
    "sh -c 'echo out; echo err >&2' >/dev/null |& cat",
    "sh -c 'echo out; echo err >&2' |&cat",
    "sh -c 'echo out; echo err >&2'|&cat",
    "sh -c 'echo out; echo err >&2' |&\ncat",
    "sh -c 'exit 3' |& cat; echo $?",
    "sh -c 'echo x >&2; exit 3' |& cat; echo $?",
    "sh -c 'echo x >&2' |& sh -c 'cat >&2' 2>&1 |& sh -c 'cat'",
    "echo x |& cat",
    "echo 'a|&b'",
    "echo \"a |& b\"",
    "echo a |&",
    "|& cat",
    "echo a |& | cat",
    "echo a |& ; echo b",
    "echo a|&cat",
    "echo a ||& cat",
    "echo a | & cat",
    "sh -c 'echo err >&2' |& cat && echo ok",
    "sh -c 'echo err >&2' |& cat; sh -c 'echo err2 >&2' |& cat",
]


def run(argv, script, cwd):
    try:
        return subprocess.run(argv + [script], cwd=cwd, env=ENV, capture_output=True, timeout=20)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, -9, b"", b"timeout")


def compare(script):
    global checked
    checked += 1
    prelude = "".join(f"{var}={shell_quote(value)}; " for var, value in VARIABLES.items())
    results = []
    for argv in (["bash", "-c"], [koa, "-c"]):
        root = tempfile.mkdtemp()
        out = run(argv, prelude + script, root)
        results.append((out, sorted(os.listdir(root))))
        shutil.rmtree(root)
    (want, files_want), (got, files_got) = results
    if want.stdout != got.stdout or want.returncode != got.returncode or files_want != files_got:
        bad.append(
            f"{script!r}: bash rc={want.returncode} {want.stdout!r} {files_want}, "
            f"koa rc={got.returncode} {got.stdout!r} {files_got} {got.stderr[:80]!r}"
        )


def compare_native(word):
    global checked
    checked += 1
    prelude = "".join(f"{var}={shell_quote(value)}; " for var, value in VARIABLES.items())
    root = tempfile.mkdtemp()
    want = run(["bash", "-c"], f"{prelude}printf '%s\\n' {word}", root)
    got = run([koa, "-c"], f"{prelude}printf '%s\\n' {word} | head -n 50 | to json", root)
    shutil.rmtree(root)
    if want.returncode != 0:
        return
    if got.returncode != 0:
        bad.append(f"{word!r}: koa did not parse it natively, rc={got.returncode} {got.stderr[:80]!r}")
        return
    texts = [row["line"] for row in json.loads(got.stdout)]
    if texts != want.stdout.decode().split("\n")[:-1]:
        bad.append(f"{word!r}: native koa gives {texts!r}, bash {want.stdout!r}")


def shell_quote(value):
    return "'" + value.replace("'", "'\\''") + "'"


for script in ANSI_SCRIPTS + PARAM_SCRIPTS + PIPE_BOTH:
    compare(script)

NATIVE_FORM = re.compile(r"\"?\$\{(#[A-Za-z0-9_?]+|[A-Za-z0-9_?]+:?[-+][^$`~'\"\\{}]*)\}\"?")
NATIVE = [f"$'{body}'" for body in ANSI_C if not re.search(r"\\n|\\r|[\n\r]", body)]
NATIVE += [word for word in NATIVE_WORDS if NATIVE_FORM.fullmatch(word)]
for word in NATIVE:
    compare_native(word)

print(f"checked {checked} scripts against bash")
for line in bad:
    print(line[:400])
sys.exit(1 if bad else 0)
