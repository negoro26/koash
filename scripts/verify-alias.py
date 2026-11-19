#!/usr/bin/env python3
import os
import shutil
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])

CASES = {
    "define and use": "alias ll='echo LL'\nll a b\n",
    "several at once": "alias a='echo A' b='echo B'\na\nb\n",
    "listing is sorted and quoted": "alias zz=1 b='it'\\''s' c=\"x y\"\nalias\nalias -p\n",
    "show one": "alias q='echo Q'\nalias q\n",
    "show missing": "alias nope\necho $?\nalias nope\n",
    "unalias one": "alias q='echo Q'\nq\nunalias q\nq\n",
    "unalias all": "alias a=1 b=2\nunalias -a\nalias\necho done\n",
    "unalias missing": "unalias nope\necho $?\n",
    "unalias usage": "unalias\necho $?\n",
    "invalid names": "alias a/b=c\necho $?\nalias 'a b'=c\necho $?\nalias 'a$b'=c\necho $?\nalias\n",
    "equals in value": "alias a=b=c\nalias a\n",
    "dash name": "alias -- -n=x\nalias\n",
    "redefine": "alias q='echo 1'\nq\nalias q='echo 2'\nq\n",
    "empty value": "alias x=''\nx echo hi\n",
    "trailing blank expands next word": "alias sudo='echo SUDO '\nalias ll='echo LL'\nsudo ll x\n",
    "trailing blank chains": "alias e='echo '\ne e e hi\n",
    "no blank no chaining": "alias w='echo'\nalias ll='echo LL'\nw ll\n",
    "self reference": "alias echo='echo X'\necho hi\n",
    "mutual reference": "alias a=b b=a\na\n",
    "after pipe": "alias p='echo P'\necho x | p\n",
    "after and or semi": "alias p='echo P'\nfalse || p && p ; p\n",
    "alias value with pipe": "alias lp='echo hi |'\nlp cat\n",
    "alias value with and": "alias both='echo 1 && echo 2'\nboth\n",
    "quoted word": "alias p='echo P'\n\"p\"\n'p'\n\\p\np\n",
    "not an argument": "alias p='echo P'\necho p\n",
    "assignment prefix": "alias p='echo P'\nX=1 p\n",
    "redirect before command": "alias p='echo P'\n>/dev/null p\np\n",
    "defined on the same line": "alias q='echo Q'; q\n",
    "defined on the next line": "alias q='echo Q'\nq\n",
    "unalias then use": "alias q='echo Q'\nunalias q\nq\n",
    "inside if": "alias q='echo Q'\nif true; then q; fi\n",
    "with command substitution": "alias q='echo Q'\nq $(echo sub)\n",
    "alias is not exported": "alias q='echo Q'\nsh -c 'q'\n",
}

bad = []


def run(argv, script, cwd):
    try:
        return subprocess.run(
            argv, input=script, cwd=cwd, capture_output=True, text=True, timeout=20
        )
    except subprocess.TimeoutExpired as err:
        return subprocess.CompletedProcess(argv, -9, (err.stdout or b"").decode(), "timeout")


for name, script in CASES.items():
    root = tempfile.mkdtemp()
    want = run(["bash", "-O", "expand_aliases"], script, root)
    got = run([koa], script, root)
    if want.stdout != got.stdout or (want.returncode == 0) != (got.returncode == 0):
        bad.append(
            f"{name}: bash rc={want.returncode} {want.stdout!r}, koa rc={got.returncode} {got.stdout!r}"
        )
    shutil.rmtree(root)

print(f"checked {len(CASES)} alias scripts against bash")
for line in bad:
    print(line)
sys.exit(1 if bad else 0)
