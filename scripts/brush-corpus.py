#!/usr/bin/env python3
"""Prototype corpus: compares `sh -c`, base koa and brush-embedded koa on bash constructs.

usage: brush-corpus.py BASE_KOA PROTO_KOA
Predicates measured: (2) stdout/stderr/status/fs equality, (3) undo restores writes with no opaque entry,
(4) every external reaches the launcher (EMBED/SPAWN trace compared with strace execve).
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

BASE, PROTO = sys.argv[1], sys.argv[2]
ENV = {
    "PATH": os.environ["PATH"],
    "HOME": os.environ.get("HOME", "/tmp"),
    "LANG": "C",
    "USER": "corpus",
    "TERM": "dumb",
}

# id, script, kind. kind: ok | expect_fail | writes (single line, undo-tested) | external_writer
CASES = [
    ("cmdsub", "echo got-$(echo sub)", "ok"),
    ("param_default", "unset v; echo ${v:-dflt} [${v:+alt}]", "ok"),
    ("backticks", "echo `echo bt`", "ok"),
    ("subshell_status", "(echo in; exit 3); echo st=$?", "ok"),
    ("heredoc", "cat <<EOF\nline $(echo x)\nEOF", "ok"),
    ("if_else", "if [ -f in.txt ]; then echo yes $(true); else echo no; fi", "ok"),
    ("for_list", "for i in 1 2 3; do echo $i; done; echo $(echo end)", "ok"),
    ("while_read", "while read l; do echo \"[$l]\"; done < in.txt; echo $(true)", "ok"),
    ("case_stmt", "x=bar; case $x in foo) echo f;; ba*) echo b$(true);; esac", "ok"),
    ("dbl_bracket", "[[ abc == a* && -f in.txt ]] && echo match $(true)", "ok"),
    ("arithmetic", "echo $((2+3*4)) $(( (1<<4) | 1 ))", "ok"),
    ("arrays", "a=(x y z); echo ${a[1]} ${#a[@]} $(true)", "ok"),
    ("pipeline_cmdsub", "echo $(printf 'b\\na\\n' | sort) | tr a-z A-Z", "ok"),
    ("here_string", "cat <<< \"hs $(echo x)\"", "ok"),
    ("proc_subst", "diff <(echo a) <(echo a) && echo same", "ok"),
    ("brace_glob", "echo $(echo {a,b}{1,2}) *.txt", "ok"),
    ("cd_pwd", "cd /tmp && echo $(pwd)", "ok"),
    ("exit_trap", "trap 'echo bye' EXIT; echo $(echo hi)", "ok"),
    ("set_e", "set -e; echo $(true); false; echo unreachable", "ok"),
    ("bg_wait", "sleep 0.1 & wait; echo done $(echo x)", "ok"),
    ("devnull_status", "echo $(echo out) >/dev/null 2>&1; echo st=$?", "ok"),
    ("stdin_pipe", "echo $(cat < in.txt | wc -l)", "ok"),
    ("func_call", "f() { echo in-f $1; }; f $(echo arg)", "ok"),
    ("export_env", "export FOO=bar; echo $(printenv FOO)", "ok"),
    ("cmd_not_found", "$(true); nonexistent-cmd-xyz; echo st=$?", "ok"),
    ("syntax_error", "echo $(true) ; if then fi", "ok"),
    ("w_for_static", "for i in 1 2 3; do echo $i > out$i; done; echo $(ls out*)", "writes"),
    ("w_append_trunc", "echo more >> old; echo new > in.txt; echo $(cat old)", "writes"),
    ("w_func", "f() { echo hi > fout; }; f; echo $(cat fout)", "writes"),
    ("w_subst_inner", "echo $(echo inner > inner.txt; echo done)", "writes"),
    ("w_if_branch", "if [ -f old ]; then echo yes > yes.txt; else echo no > no.txt; fi; echo $(true)", "writes"),
    ("w_case_branch", "x=b; case $x in a) echo A > ca;; b) echo B > cb;; esac; echo $(true)", "writes"),
    ("w_var_target", "t=vt.txt; echo data > $t; echo $(cat $t)", "writes"),
    ("w_group_redirect", "{ echo one; echo $(echo two); } > grp.txt; cat grp.txt", "writes"),
    ("w_exec_fd", "exec 3> f3; echo hi >&3; exec 3>&-; echo $(cat f3)", "writes"),
    ("w_both", "echo $(echo e) &> both.txt; echo $(cat both.txt)", "writes"),
    ("w_cd_relative", "cd sub && echo x > rel.txt; echo $(true)", "writes"),
    ("w_counter_loop", "n=0; while [ $n -lt 3 ]; do echo $n > w$n; n=$((n+1)); done; echo $(ls w*)", "writes"),
    ("w_read_loop", "while read l; do echo x > \"p_$l\"; done < in.txt; echo $(true)", "writes"),
    ("w_glob_for", "for f in *.txt; do echo x > $f.bak; done; echo $(true)", "writes"),
    ("w_eval_dynamic", "c=\"echo hi > ev.txt\"; eval \"$c\"; echo $(true)", "writes"),
    ("w_tee_external", "echo hi | tee t.txt; echo $(true)", "external_writer"),
    ("x_select", "select x in a b; do echo \"sel=$x\"; break; done <<< 1", "expect_fail"),
    ("x_wait_n", "sleep 0.1 & sleep 0.3 & wait -n; echo first-done $(true); wait", "expect_fail"),
    ("x_bang", "sleep 0.2 & p=$!; [ -n \"$p\" ] && echo has-pid || echo no-pid; wait", "expect_fail"),
]


def fixture(root):
    d = tempfile.mkdtemp(prefix="koa-corpus-", dir=root)
    with open(os.path.join(d, "in.txt"), "w") as f:
        f.write("a\nb\nc\n")
    os.mkdir(os.path.join(d, "sub"))
    with open(os.path.join(d, "old"), "w") as f:
        f.write("orig\n")
    return d


def snap(d):
    out = {}
    for base, _dirs, files in os.walk(d):
        for name in files:
            p = os.path.join(base, name)
            rel = os.path.relpath(p, d)
            try:
                with open(p, "rb") as f:
                    out[rel] = f.read()
            except OSError:
                out[rel] = b"<unreadable>"
    return out


def norm(text, d):
    return text.replace(d, "$D")


def run(argv, d, stdin=None, env=None, timeout=15):
    p = subprocess.run(
        argv,
        cwd=d,
        env=env or ENV,
        input=stdin,
        capture_output=True,
        timeout=timeout,
    )
    return p.returncode, p.stdout.decode(errors="replace"), p.stderr.decode(errors="replace")


def three_way(script, root):
    res = {}
    for label, argv in (
        ("sh", ["/bin/sh", "-c", script]),
        ("base", [BASE, "-c", script]),
        ("proto", [PROTO, "-c", script]),
    ):
        d = fixture(root)
        try:
            rc, out, err = run(argv, d, stdin=b"")
            res[label] = {
                "rc": rc,
                "stdout": norm(out, d),
                "stderr": norm(err, d),
                "fs": {k: v.decode(errors="replace") for k, v in snap(d).items()},
            }
        except subprocess.TimeoutExpired:
            res[label] = {"rc": "timeout", "stdout": "", "stderr": "", "fs": {}}
    return res


def diff_fields(a, b):
    return [k for k in ("stdout", "stderr", "rc", "fs") if a[k] != b[k]]


def undo_check(script, root, allow_abs=False):
    """Runs `script` as a stdin line, then undo and tx-log, in a fresh fixture."""
    d = fixture(root)
    before = snap(d)
    one_line = script.replace("\n", "; ")
    stdin = (one_line + "\nundo\ntx-log\n").encode()
    rc, out, err = run([PROTO], d, stdin=stdin)
    after = snap(d)
    ok_restored = after == before
    opaque = re.search(r"E_TX_OPAQUE", err) is not None
    opaque_col = None
    for line in out.splitlines():
        if line.startswith("-----") or line.startswith("command"):
            continue
    return {
        "restored": ok_restored,
        "opaque_error": opaque,
        "stderr": norm(err, d).strip()[:300],
        "tx_log": norm(out, d).strip()[-500:],
        "diff": sorted(set(after) ^ set(before)) + [k for k in before if k in after and before[k] != after[k]],
    }


def trace_check(script, root):
    """Compares koa's EMBED/SPAWN trace with the execve calls strace sees from the proto binary."""
    d = fixture(root)
    trace = os.path.join(d, ".trace")
    strace_out = os.path.join(root, "strace.out")
    if os.path.exists(strace_out):
        os.remove(strace_out)
    env = dict(ENV)
    env["KOA_TRACE_SPAWN"] = trace
    try:
        run(
            ["strace", "-f", "-qq", "-e", "trace=execve", "-o", strace_out, "-s", "200", PROTO, "-c", script],
            d,
            stdin=b"",
            env=env,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None
    spawns = []
    embeds = 0
    if os.path.exists(trace):
        for line in open(trace):
            if line.startswith("SPAWN\t"):
                spawns.append(line.split("\t")[1])
            elif line.startswith("EMBED\t"):
                embeds += 1
    execs = []
    if os.path.exists(strace_out):
        for line in open(strace_out):
            m = re.search(r'execve\("([^"]+)"', line)
            if m and "= 0" in line:
                execs.append(m.group(1))
    # the first execve is koa itself
    execs = execs[1:]
    return {"embeds": embeds, "spawns": spawns, "execves": execs}


def main():
    root = tempfile.mkdtemp(prefix="koa-corpus-root-")
    report = {"cases": {}}
    try:
        for cid, script, kind in CASES:
            r = three_way(script, root)
            entry = {"kind": kind, "script": script}
            entry["proto_vs_sh"] = diff_fields(r["sh"], r["proto"])
            entry["proto_vs_base"] = diff_fields(r["base"], r["proto"])
            entry["base_vs_sh"] = diff_fields(r["sh"], r["base"])
            entry["sh"] = {k: r["sh"][k] for k in ("rc", "stdout", "stderr")}
            entry["proto"] = {k: r["proto"][k] for k in ("rc", "stdout", "stderr")}
            if kind in ("writes", "writes_abs", "external_writer"):
                entry["undo"] = undo_check(script, root)
            entry["trace"] = trace_check(script, root)
            report["cases"][cid] = entry
    finally:
        shutil.rmtree(root, ignore_errors=True)
    json.dump(report, sys.stdout, indent=1)


main()
