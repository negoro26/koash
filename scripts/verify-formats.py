#!/usr/bin/env python3
import csv
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
bad = []
checked = 0

CELLS = [
    "plain", "with,comma", 'with "quote"', "line\nbreak", "crlf\r\nbreak", "tab\there", "", " lead", "trail ",
    "\u00fcn\u00ef \u2603 \u65e5\u672c\u8a9e", "\U0001f600 face", '""', ",", '"', "\\", "a|b", "|", "'single'",
    ",\n\"", "x,y\tz", "end\n", "\nstart",
]
CELLS_NO_TAB = [cell for cell in CELLS if "\t" not in cell]


def expect(ok, message):
    global checked
    checked += 1
    if not ok:
        bad.append(message)


def run_koa(script, cwd, stdin=None):
    return subprocess.run(
        [koa, "-c", script], cwd=cwd, input=stdin, capture_output=True, timeout=30
    )


def koa_json(script, cwd, stdin=None):
    out = run_koa(script, cwd, stdin)
    if out.returncode != 0:
        bad.append(f"koa -c {script!r} exited {out.returncode}: {out.stderr.decode(errors='replace').strip()}")
        return None
    return json.loads(out.stdout)


def write_json(path, rows):
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(rows, fh, ensure_ascii=False)


def python_csv(rows, columns, delimiter, terminator):
    buf = io.StringIO(newline="")
    writer = csv.writer(buf, delimiter=delimiter, lineterminator=terminator)
    writer.writerow(columns)
    writer.writerows([[row[c] for c in columns] for row in rows])
    return buf.getvalue()


def python_read(data, delimiter):
    return [r for r in csv.reader(io.StringIO(data, newline=""), delimiter=delimiter) if r]


def tables():
    wide = [
        {"a_col": CELLS[i], "b_col": CELLS[(i + 3) % len(CELLS)], "c_col": CELLS[(i + 7) % len(CELLS)]}
        for i in range(len(CELLS))
    ]
    yield "wide", ["a_col", "b_col", "c_col"], wide
    for cell in CELLS:
        yield f"one column {cell!r}", ["a_col"], [{"a_col": cell}, {"a_col": "next"}]
    for cell in CELLS:
        yield f"only {cell!r}", ["a_col"], [{"a_col": cell}]


root = tempfile.mkdtemp()

for label, columns, rows in tables():
    write_json(os.path.join(root, "t.json"), rows)
    source = "cat --raw t.json | from json"
    for fmt, delimiter in (("csv", ","), ("tsv", "\t")):
        out = run_koa(f"{source} | to {fmt}", root)
        text = out.stdout.decode("utf-8")
        want = [columns, *[[row[c] for c in columns] for row in rows]]
        got = python_read(text, delimiter)
        expect(out.returncode == 0 and got == want, f"to {fmt} {label}: python reads {got!r}, want {want!r}")

        for terminator in ("\n", "\r\n"):
            data = python_csv(rows, columns, delimiter, terminator)
            parsed = koa_json(f"from {fmt} | to json", root, data.encode())
            back = [{c: ("" if r.get(c) is None else str(r[c])) for c in columns} for r in (parsed or [])]
            numeric_free = all(not cell.strip().lstrip("-").isdigit() for row in rows for cell in row.values())
            if numeric_free:
                expect(back == rows, f"from {fmt} {label} with {terminator!r} line ends: koa reads {back!r}, want {rows!r}")

        twice = run_koa(f"{source} | to {fmt} | from {fmt} | to {fmt}", root)
        expect(twice.stdout == out.stdout, f"{fmt} round trip {label}: {twice.stdout!r} != {out.stdout!r}")
        again = koa_json(f"{source} | to {fmt} | from {fmt} | to json", root)
        expect(again == koa_json(f"{source} | to json", root), f"{fmt} round trip {label} changed the table: {again!r}")

for label, columns, rows in tables():
    if not label.startswith("wide"):
        continue
    write_json(os.path.join(root, "t.json"), rows)
    out = run_koa("cat --raw t.json | from json | to md", root).stdout.decode()
    cell = lambda text: text.replace("\\", "\\\\").replace("|", "\\|").replace("\r", "").replace("\n", " ")
    want = "".join(
        [
            "| " + " | ".join(columns) + " |\n",
            "| " + " | ".join("---" for _ in columns) + " |\n",
            *["| " + " | ".join(cell(row[c]) for c in columns) + " |\n" for row in rows],
        ]
    )
    expect(out == want, f"to md: koa {out!r}, want {want!r}")

TYPED = "name,count,ratio,flag,note\nalpha,7,0.5,true,\nbeta,-3,2.25,false,x y\n"
got = koa_json("from csv | to json", root, TYPED.encode())
want = [
    {"name": "alpha", "count": 7, "ratio": 0.5, "flag": True, "note": ""},
    {"name": "beta", "count": -3, "ratio": 2.25, "flag": False, "note": "x y"},
]
expect(got == want, f"csv type inference: koa {got!r}, want {want!r}")

RAGGED = "a,b,c\n1,2\n3,4,5,6\n\n7,8,9\n"
got = koa_json("from csv | to json", root, RAGGED.encode())
expect(
    got == [{"a": 1, "b": 2}, {"a": 3, "b": 4, "c": 5}, {"a": 7, "b": 8, "c": 9}],
    f"ragged csv: koa {got!r}",
)
got = koa_json("from csv | to json", root, b"a,b\r\n1,2\r\n3,4")
expect(got == [{"a": 1, "b": 2}, {"a": 3, "b": 4}], f"crlf and no final newline: koa {got!r}")
got = koa_json("from csv | to json", root, b"id,n\n12345678901234567890123,-98765432109876543210\n")
expect(
    got == [{"id": "12345678901234567890123", "n": "-98765432109876543210"}],
    f"integers too long for 64 bits stay text: koa {got!r}",
)
got = koa_json("from csv | select a | to json", root, b"\xef\xbb\xbfa,b\n1,2\n")
expect(got == [{"a": 1}], f"a UTF-8 BOM is not part of the first column name: koa {got!r}")
got = run_koa("from json | to md", root, b'[{"c":"x\\\\|y"}]').stdout.decode()
expect(got.endswith("| x\\\\\\|y |\n"), f"to md escapes a backslash before a pipe: koa {got!r}")

LINE_INPUTS = [
    b"", b"one", b"one\n", b"one\ntwo\n", b"one\n\nthree\n", b"\n", b"\n\n", b"x\ny", "\u00fcn\u00ef\n\u65e5\u672c\n".encode(),
    b"tab\there\n", b"trailing space \n", b"a\r\nb\r\n",
]
for data in LINE_INPUTS:
    want = subprocess.run(
        ["bash", "-c", 'mapfile -t a; printf "%s\\0" "${a[@]}"'], input=data, capture_output=True
    ).stdout
    want = [] if want == b"\0" and data == b"" else want.decode().split("\0")[:-1]
    want = [line.removesuffix("\r") for line in want]
    got = koa_json("from lines | to json", root, data)
    texts = [r["line"] for r in got] if got else []
    expect(texts == want, f"from lines {data!r}: koa {texts!r}, bash mapfile {want!r}")

shutil.rmtree(root)
print(f"checked {checked} csv, tsv, md and lines conversions against python csv and bash")
for line in bad:
    print(line[:300])
sys.exit(1 if bad else 0)
