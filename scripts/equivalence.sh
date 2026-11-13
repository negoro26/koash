#!/usr/bin/env bash
# Behavior pin for builtin refactors.
# Usage: scripts/equivalence.sh <old-koa> <new-koa>
# Runs a fixed list of commands through both binaries in identically named
# fresh scratch dirs and diffs stdout, stderr, exit code, and the resulting
# file tree. Exits non-zero and prints each command that differs.
set -uo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <old-koa> <new-koa>" >&2
    exit 2
fi
OLD=$(realpath "$1")
NEW=$(realpath "$2")

ROOT=/tmp/koa-equiv
SCRATCH=$ROOT/work
HOME_DIR=$ROOT/home
OUT=$ROOT/out
STAMP=1700000000

KINDS=()
CMDS=()
INPUTS=()

add() { KINDS+=("$1"); CMDS+=("$2"); INPUTS+=("$3"); }
c() { add c "$1" ""; }
s() { add c "$2" "$1"; }
t() { add t "$1" ""; }
r() { add r "$1" ""; }

fnv_tag() {
    local h=$((0x811c9dc5)) i byte
    for ((i = 0; i < ${#1}; i++)); do
        printf -v byte '%d' "'${1:i:1}"
        h=$(((h ^ byte) * 0x01000193 & 0xffffffff))
    done
    printf '%03x' $((h & 0xfff))
}

fixtures() {
    rm -rf "$ROOT/work" "$ROOT/home"
    mkdir -p "$SCRATCH/sub/deep" "$SCRATCH/proj/src" "$SCRATCH/empty_dir" "$HOME_DIR"
    cd "$SCRATCH" || exit 2
    printf 'alpha\nbeta\ngamma\ndelta\nepsilon\n' >a.txt
    printf 'one\ntwo\nthree\n' >b.txt
    : >empty.txt
    printf 'no newline at end' >nonl.txt
    printf 'fn main() {\n    let x = 1;\n    println!("{x}");\n}\n' >sub/c.rs
    printf '# title\n\nsome Alpha text\n' >sub/deep/d.md
    printf 'secret\n' >.hidden
    printf '[{"name":"x","size":3},{"name":"y","size":10},{"name":"z","size":7}]\n' >data.json
    printf '{"k":1,"v":"a"}\n{"k":2,"v":"b"}\n' >lines.json
    printf '\xff\xfe\x00bin\n' >bin.dat
    printf 'pub fn compute() -> u32 {\n    helper() + 1\n}\nfn helper() -> u32 { 41 }\n' >proj/src/lib.rs
    printf 'use proj::compute;\nfn main() { compute(); }\n' >proj/src/main.rs
    ln -s a.txt link
    find . -depth -exec touch -h -d "@$STAMP" {} +
}

listing() {
    cd "$SCRATCH" || return
    find . -printf '%p %y %m %s\n' | LC_ALL=C sort
    find . -type f -exec sha256sum {} + | LC_ALL=C sort
}

normalize() {
    sed -E \
        -e 's/^\[([0-9]+)\] [0-9]+$/[\1] <pid>/' \
        -e 's/"duration_ms":[0-9]+/"duration_ms":0/g' \
        -e 's/"pid":[0-9]+/"pid":0/g'
}

run_one() {
    local bin=$1 dest=$2 kind=$3 cmd=$4 input=$5
    mkdir -p "$dest"
    fixtures
    cd "$SCRATCH" || exit 2
    printf '%s' "$input" >"$ROOT/stdin"
    local runner=(env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin TERM=dumb LANG=C)
    case $kind in
        c) timeout 30 "${runner[@]}" "$bin" -c "$cmd" <"$ROOT/stdin" >"$dest/stdout.raw" 2>"$dest/stderr.raw" ;;
        t) timeout 30 "${runner[@]}" "$bin" --tsp -c "$cmd" <"$ROOT/stdin" >"$dest/stdout.raw" 2>"$dest/stderr.raw" ;;
        r) timeout 60 "${runner[@]}" "$bin" --rpc <<<"$cmd" >"$dest/stdout.raw" 2>"$dest/stderr.raw" ;;
    esac
    echo $? >"$dest/code"
    normalize <"$dest/stdout.raw" >"$dest/stdout"
    normalize <"$dest/stderr.raw" >"$dest/stderr"
    rm -f "$dest/stdout.raw" "$dest/stderr.raw"
    listing >"$dest/files" 2>&1
}

rpc() {
    local method=$1 params=$2
    printf '{"id":%s,"method":"%s","params":%s}\n' "$RPC_ID" "$method" "$params"
    RPC_ID=$((RPC_ID + 1))
}

rpc_script() {
    RPC_ID=2
    printf '{"id":1,"method":"session.create"}\n'
    local cmd
    for cmd in "$@"; do
        rpc exec "{\"session_id\":\"sess-1\",\"command\":$(printf '%s' "$cmd" | jq -Rs .)}"
    done
}

source_cases() {
    # discovery and help
    c 'help'
    c 'help ls'
    c 'help sort'
    c 'help nonexistent-thing'
    c 'help | select command'
    c 'discover search code hash'
    c 'discover list files'
    c 'discover --limit 8 process'
    c 'discover'
    c 'discover -x'
    c 'discover undo transaction'
    t 'help'

    # navigation and environment
    c 'pwd'
    c 'cd sub && pwd'
    c 'cd sub; cd -; pwd'
    c 'cd -'
    c 'cd nope'
    c 'cd a.txt'
    c 'cd -- sub && pwd'
    c 'cd && pwd'
    c 'export FOO=bar; env | where key == FOO'
    c 'export A=1 B=2 C; env | where key == B'
    c 'env | where key == HOME'
    c 'FOO=1 env | where key == FOO'
    c 'X=5; env | where key == X'
    c 'echo $HOME'
    c 'cd sub; env | where key == OLDPWD | select value'

    # listing and metadata
    c 'ls'
    c 'ls -a'
    c 'ls -l'
    c 'ls -la sub'
    c 'ls --author'
    c 'ls sub a.txt'
    c 'ls -- -x'
    c 'ls nope'
    c 'ls | where size > 10 | select name size'
    c 'ls | sort-by size'
    c 'ls | sort_by --desc name'
    c 'ls | sort-by -r name | first 2'
    c 'ls | last 2'
    c 'ls | first'
    c 'ls | last'
    c 'ls | select name kind | to json'
    c 'ls | to compact'
    t 'ls | select name size'
    c 'ls proj | select name'
    c 'stat a.txt'
    c 'stat a.txt sub link'
    c 'stat'
    c 'stat nope'
    c 'find'
    c 'find sub'
    c 'find . -type f'
    c 'find -t d'
    c 'find -type dir sub'
    c 'find -name "*.rs"'
    c 'find -iname "*.MD"'
    c 'find --hidden'
    c 'find -maxdepth 1'
    c 'find -maxdepth x'
    c 'find -name "[" '
    c 'find proj/ -name "*.rs"'
    c 'find nope'

    # file contents
    c 'cat a.txt'
    c 'cat --raw a.txt'
    c 'cat -r a.txt b.txt'
    c 'cat -n a.txt'
    c 'cat a.txt b.txt'
    c 'cat nope.txt'
    c 'cat bin.dat'
    c 'cat --raw bin.dat'
    c 'cat empty.txt'
    c 'cat nonl.txt'
    c 'cat -- -r'
    c 'cat'
    s $'from stdin\nsecond\n' 'cat'
    s $'from stdin\nsecond\n' 'cat -'
    s $'from stdin\nsecond\n' 'cat a.txt - b.txt'
    s $'from stdin\nsecond\n' 'cat --raw -'
    c 'cat < b.txt'
    c 'cat a.txt | select line text'
    c 'cat a.txt | first 2 | select hash'
    c 'cat a.txt | to compact'
    c 'cat a.txt | to json'
    c 'cat a.txt | to xml'
    c 'head a.txt'
    c 'head -n 2 a.txt'
    c 'head -n2 a.txt'
    c 'head -3 a.txt'
    c 'head -n 2 --raw a.txt'
    c 'head -n 1 a.txt b.txt'
    c 'head -n x a.txt'
    c 'head nope.txt'
    c 'head -n 0 a.txt'
    s $'1\n2\n3\n4\n' 'head -n 2'
    s $'1\n2\n3\n4\n' 'head -2'
    s '' 'head'
    c 'head < a.txt'
    c 'ls | head -n 2 | select name'
    c 'tail a.txt'
    c 'tail -n 2 a.txt'
    c 'tail -2 a.txt'
    c 'tail -n2 --raw a.txt b.txt'
    c 'tail nope.txt'
    s $'1\n2\n3\n4\n' 'tail -n 2'
    s $'1\n2\n3\n4\n' 'tail'
    c 'tail < a.txt'
    c 'ls | tail -n 2 | select name'
    c 'grep alpha a.txt'
    c 'grep -i ALPHA a.txt'
    c 'grep -n beta a.txt b.txt'
    c 'grep -c a a.txt'
    c 'grep -l a a.txt b.txt sub'
    c 'grep -v a a.txt'
    c 'grep -F "a.b" a.txt'
    c 'grep -w alpha a.txt'
    c 'grep -r fn sub'
    c 'grep -rn Alpha .'
    c 'grep -R compute proj --include "*.rs"'
    c 'grep -r fn . --exclude "*.rs"'
    c 'grep -r fn .'
    c 'grep -rc let sub'
    c 'grep -rl e sub proj'
    c 'grep'
    c 'grep "(" a.txt'
    c 'grep -E "a+" a.txt'
    c 'grep --include'
    c 'grep x nope.txt'
    c 'grep -r x nope'
    s $'alpha\nbeta\n' 'grep alpha'
    s $'alpha\nbeta\n' 'grep -c a'
    s $'alpha\nbeta\n' 'grep -l a'
    s $'alpha\nbeta\n' 'grep -v alpha'
    s $'alpha\nbeta\n' 'grep -r alpha'
    c 'cat a.txt | grep a'
    c 'cat a.txt | grep -c a'
    c 'cat a.txt | grep -l a'
    c 'cat a.txt b.txt | grep -l o'
    c 'ls | grep a'
    c 'ls | grep -c a'
    c 'find | grep -c rs'
    c 'find -name "*.rs" | grep fn'
    c 'echo abc | grep b'
    c 'echo abc | grep -c b'
    c 'echo abc | grep -l b'
    c 'echo abc | grep zzz'
    c 'grep -r alpha < a.txt'
    c 'grep alpha < a.txt'

    # printing
    c 'echo hello world'
    c 'echo -n hi'
    c 'echo -n -n hi'
    c 'echo a -n b'
    c 'echo'
    c 'echo "a  b"   c'
    c 'echo hi > out.txt; cat out.txt'
    c 'echo hi >> out.txt; echo again >> out.txt; cat --raw out.txt'
    c 'ls | select name > names.txt; cat --raw names.txt'

    # editing with hash anchors
    c 'patch a.txt 2:xxx "BETA"'
    c 'patch a.txt'
    c 'patch a.txt 2'
    c 'patch a.txt x:abc y'
    c 'patch a.txt 0:abc y'
    c 'patch a.txt 99:abc y'
    c 'patch a.txt 2:abc'
    c 'patch nope.txt 1:abc y'
    c 'cat a.txt | where line == 2 | select hash'
    c 'cat a.txt | where line == 3 | select hash | to json'
    c 'cat a.txt | where line == 2 | select text'
    c 'patch a.txt 2:BETAHASH "BETA"; cat a.txt'
    c 'patch a.txt 2:BETAHASH --delete; cat a.txt'
    c 'patch a.txt 2:BETAHASH "x"; undo; cat a.txt'
    c 'patch a.txt 2:BETAHASH "x"; patch a.txt 2:BETAHASH "y"'

    # filesystem mutation
    c 'mkdir newdir; ls | select name'
    c 'mkdir -p x/y/z; find x'
    c 'mkdir x/y'
    c 'mkdir sub'
    c 'mkdir -p sub; find sub | select path'
    c 'mkdir -- -p; ls -a | select name'
    c 'mkdir'
    c 'mkdir a b c; ls | select name'
    c 'rm a.txt; ls | select name'
    c 'rm nope'
    c 'rm -f nope'
    c 'rm --force a.txt; find -maxdepth 1 | select path'
    c 'rm sub'
    c 'rm empty_dir; ls | select name'
    c 'rm -r sub; ls | select name'
    c 'rm -rf sub proj; ls | select name'
    c 'rm --force sub; ls | select name'
    c 'rm a.txt b.txt; ls | select name'
    c 'rm'
    c 'cp a.txt copy.txt; cat --raw copy.txt'
    c 'cp a.txt b.txt; cat --raw b.txt'
    c 'cp a.txt b.txt dst; ls | select name'
    c 'cp a.txt sub; ls sub | select name'
    c 'cp sub sub2'
    c 'cp -r sub sub2; find sub2 | select path'
    c 'cp -R sub empty_dir; find empty_dir | select path'
    c 'cp a.txt'
    c 'cp nope x'
    c 'cp -r proj proj2; grep -r compute proj2 | select file'
    c 'mv a.txt moved.txt; ls | select name'
    c 'mv a.txt sub; ls sub | select name'
    c 'mv nope x'
    c 'mv a.txt'
    c 'mv sub empty_dir; find empty_dir | select path'
    c 'rename compute calc; cat --raw proj/src/lib.rs'
    c 'rename compute'
    c 'rename'
    c 'rename nothing_here_xyz abc'

    # tx ledger
    c 'tx-log'
    c 'tx_log'
    c 'undo'
    c 'rollback'
    c 'rollback tx_999'
    c 'rm a.txt; tx-log'
    c 'rm a.txt; undo; ls | select name'
    c 'rm a.txt; undo; undo'
    c 'rm a.txt; mkdir z; tx_log'
    c 'rm a.txt; mkdir z; rollback tx_001; ls | select name'
    c 'rm a.txt; mkdir z; rollback tx_002; ls | select name'
    c 'mkdir z; mv z zz; undo; ls | select name'
    c 'cp a.txt c.txt; undo; ls | select name'
    c 'echo x > o.txt; undo; ls | select name'
    c 'rename compute calc; tx-log; undo; cat --raw proj/src/lib.rs'
    c 'rm a.txt | cat'
    c 'undo | select tx_id'
    c 'tx-log | to json'
    c 'rm a.txt && tx-log | select command files'
    c 'rm a.txt; mkdir z; tx-log | where files > 0'

    # table verbs
    c 'cat data.json | from json'
    c 'cat --raw data.json | from json | where size > 5'
    c 'cat --raw data.json | from json | where size >= 7 | select name'
    c 'cat --raw data.json | from json | where name == x'
    c 'cat --raw data.json | from json | where name != x | select name'
    c 'cat --raw data.json | from json | where name contains y'
    c 'cat --raw data.json | from json | where name =~ z'
    c 'cat --raw data.json | from json | where name > x'
    c 'cat --raw data.json | from json | where size ?? 1'
    c 'cat --raw data.json | from json | where nope == 1'
    c 'cat --raw data.json | from json | where size 1'
    c 'cat --raw data.json | from json | where size > 1kb'
    c 'cat --raw data.json | from json | sort-by size'
    c 'cat --raw data.json | from json | sort-by --desc size'
    c 'cat --raw data.json | from json | sort-by -r name'
    c 'cat --raw data.json | from json | sort-by nope'
    c 'cat --raw data.json | from json | sort-by'
    c 'cat --raw data.json | from json | first 2'
    c 'cat --raw data.json | from json | last 2'
    c 'cat --raw data.json | from json | first x'
    c 'cat --raw data.json | from json | select name'
    c 'cat --raw data.json | from json | select nope'
    c 'cat --raw data.json | from json | select'
    c 'cat --raw data.json | from json | to json'
    c 'cat --raw data.json | from json | to compact'
    c 'cat --raw lines.json | from json'
    c 'cat --raw lines.json | from json | to compact'
    c 'cat --raw a.txt | from json'
    c 'cat --raw empty.txt | from json'
    c 'cat --raw a.txt | from yaml'
    c 'from json < data.json'
    c 'from'
    c 'to'
    c 'where'
    c 'where a b'
    c 'select'
    c 'sort-by'
    c 'first'
    c 'last 3'
    s '[{"q":1}]' 'from json'
    s '[{"q":1}]' 'from json | to compact'
    s '[{"q":1},{"q":2}]' 'from json | where q > 1'
    s '[{"q":1},{"q":2}]' 'from json | sort_by -r q | first 1'
    s '[{"q":1}]' 'where q == 1'
    s '[{"q":1}]' 'first'
    s '[{"q":1}]' 'to json'

    # processes and executables
    c 'ps | first 0'
    c 'ps | where pid == 1 | select pid'
    c 'ps | to compact | head -n 0'
    c 'which sh'
    c 'which sh nope-cmd-xyz ls'
    c 'which'
    c 'kill'
    c 'kill abc'
    c 'kill -s'
    c 'kill -s FOO 1'
    c 'kill 999999999'
    c 'kill -9 999999999'
    c 'kill -15 999999999'
    c 'kill -s TERM 999999999'
    c 'kill -s sigint 999999999'
    c 'kill -s HUP 999999999'
    c 'kill -s STOP 999999999'
    c 'kill -s CONT 999999999'
    c 'kill -s KILL 999999999'

    # diagnostics and code intelligence
    c 'diag'
    c 'refs'
    c 'refs compute'
    c 'refs helper'
    c 'refs "("'
    c 'refs compute | select file line'
    c 'prewalk'
    c 'prewalk --budget 40'
    c 'prewalk --budget x'
    c 'prewalk proj'
    c 'ast'
    c 'ast compute'
    c 'ast main'
    c 'ast helper proj'
    c 'ast nothing_here_xyz'

    # sub-kernels
    c '@sql "select 1 as one, 2 as two"'
    c '@sql "create table t(a int); insert into t values (1),(2); select * from t"'
    c '@sql "select * from missing"'
    c '@sql'
    c '@sql select 40 + 2 as answer'
    c '@py "print(40 + 2)"'
    c '@py "import os; print(os.getcwd())"'
    c '@py "raise ValueError(1)"'
    c '@py "x = 5"; @py "print(x)"'
    c '@py "print(1)"; tx-log'
    c '@js "console.log(40 + 2)"'
    c '@js "throw new Error(1)"'
    c '@js "console.log(1)"; tx-log'

    # builtins that run alone
    c 'jobs'
    c 'fg'
    c 'bg'
    c 'fg 7'
    c 'bg 7'
    c 'fg %1'
    c 'jobs | cat'
    c 'jobs &'
    c 'exit'
    c 'exit 3'
    c 'exit abc'
    c 'echo before; exit 4; echo after'
    c 'false; exit'
    c 'exit 300'
    c 'exit 3 | cat'
    c 'exit 3 &'
    c 'FOO=1 exit 5'
    c 'exit 3 > out.txt'
    c 'true && exit 6'
    c 'false || exit 7'
    c 'unset'
    c 'unset FOO'
    c 'export FOO=1; unset FOO; env | where key == FOO'
    c 'export FOO=1 BAR=2; unset FOO BAR; env | where key == BAR'
    c 'unset PATH; env | where key == PATH'
    c 'unset FOO | cat'
    c 'unset FOO &'
    c 'sleep 1 & jobs'
    c 'sleep 1 & jobs; fg 9'
    c 'cat a.txt | jobs'

    # executor behavior around builtins
    c 'ls &'
    c 'ls | /bin/cat'
    c '/bin/echo external'
    c 'cat a.txt | /bin/cat | first 1'
    c 'nonexistent-command-xyz'
    c 'nonexistent-command-xyz | cat'
    c 'cat a.txt | head -n 1 | tail -n 1'
    c 'echo a && echo b || echo c'
    c 'false && echo skipped; echo ran'
    c 'ls nope || echo recovered'
    c 'cd sub; ls | select name; cd ..; pwd'
    c 'echo hi | cat'
    c 'sh -c "echo from sh; exit 3"'
    c 'sort_by'
    c 'sort-by name | first'
    c 'curl --version > /dev/null; echo done'
    c 'if true; then echo posix; fi'
    c 'for i in 1 2; do echo $i; done'
    c 'echo $(echo sub)'
    c 'true; echo $?'

    # protocol
    r "$(printf '{"id":1,"method":"session.create"}\n{"id":2,"method":"session.schemas"}\n')"
    r "$(rpc_script 'ls | select name' 'rm a.txt' 'tx-log' 'undo' 'ls | select name')"
    r "$(rpc_script 'help' 'discover process' 'jobs' 'exit 3' 'echo after')"
    r "$(rpc_script 'cat a.txt | head -n 2' 'grep -c a a.txt' 'cat nope' 'sort_by')"
    r "$(rpc_script 'echo hi | cat' '@sql "select 1 as one"' '@py "print(1)"' 'tx-log')"
}

main() {
    rm -rf "$OUT"
    mkdir -p "$OUT"
    source_cases
    local total=${#CMDS[@]} diffs=0 i kind cmd input
    for ((i = 0; i < total; i++)); do
        kind=${KINDS[$i]}
        cmd=${CMDS[$i]//BETAHASH/$BETA_HASH}
        input=${INPUTS[$i]}
        run_one "$OLD" "$OUT/$i/old" "$kind" "$cmd" "$input"
        run_one "$NEW" "$OUT/$i/new" "$kind" "$cmd" "$input"
        if ! diff -ru "$OUT/$i/old" "$OUT/$i/new" >"$OUT/$i/diff"; then
            diffs=$((diffs + 1))
            printf 'DIFF [%s] %s\n' "$kind" "$cmd"
            sed -n '1,40p' "$OUT/$i/diff"
        fi
    done
    printf 'commands: %d, differences: %d\n' "$total" "$diffs"
    [ "$diffs" -eq 0 ]
}

BETA_HASH=$(fnv_tag beta)

main
