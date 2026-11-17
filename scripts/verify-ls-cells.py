#!/usr/bin/env python3
import datetime
import json
import os
import random
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
rng = random.Random(0)
first = datetime.date(1960, 1, 1).toordinal()
last = datetime.date(2100, 12, 31).toordinal()
far = datetime.date(2400, 12, 31).toordinal()
epoch = datetime.date(1970, 1, 1).toordinal()


def offset(day):
    date = datetime.date.fromordinal(day)
    if date.day == 1 or (date.month == 2 and date.day == 29):
        return 0
    if date.day == 2:
        return 86_399
    return rng.randrange(86_400)


def koa_json(directory, script):
    out = subprocess.run([koa, "-c", f"{script} | to json"], cwd=directory, capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"koa failed on {script!r}: {out.stderr}")
    return json.loads(out.stdout or "[]")


def column(directory, script, key):
    return [row.get(key, "") for row in koa_json(directory, script)]


with tempfile.TemporaryDirectory() as root:
    days = os.path.join(root, "days")
    modes = os.path.join(root, "modes")
    os.mkdir(days)
    os.mkdir(modes)
    expected = {}
    sampled = rng.sample(range(last + 1, far + 1), 5000)
    for day in [*range(first, last + 1), *sampled]:
        t = (day - epoch) * 86_400 + offset(day)
        name = f"d{day}"
        path = os.path.join(days, name)
        open(path, "w").close()
        os.utime(path, (t, t))
        if t < 0:
            expected[name] = ""
        else:
            stamp = datetime.datetime.fromtimestamp(t, datetime.timezone.utc)
            expected[name] = stamp.strftime("%Y-%m-%dT%H:%M:%SZ")
    for mode in range(0o10000):
        path = os.path.join(modes, f"m{mode}")
        open(path, "w").close()
        os.chmod(path, mode)

    bad = []
    rows = koa_json(days, "ls | select name modified")
    seen = {row["name"]: row.get("modified", "") for row in rows}
    for name, want in expected.items():
        if seen.get(name) != want:
            bad.append(f"{name}: modified {seen.get(name)!r}, expected {want!r}")
    rows = koa_json(modes, "ls | select name permissions")
    for row in rows:
        want = format(int(row["name"][1:]), "o")
        if row["permissions"] != want:
            bad.append(f"{row['name']}: permissions {row['permissions']!r}, expected {want!r}")
    if len(rows) != 0o10000:
        bad.append(f"ls listed {len(rows)} of {0o10000} mode files")

    filters = 0
    for directory, key, values in [
        (days, "modified", ["", "1969-12-31T23:59:59Z", "1970-01-01T00:00:00Z", "2000-02-29T00:00:00Z", "2100-12-31T23:59:59Z"]),
        (modes, "permissions", ["0", "755", "4755", "7777"]),
    ]:
        listed = koa_json(directory, f"ls | select name {key}")
        text = {row["name"]: row.get(key, "") for row in listed}
        order = [row["name"] for row in listed]
        want = sorted(order, key=lambda n: text[n])
        got = column(directory, f"ls | sort-by {key} | select name", "name")
        filters += 1
        if got != want:
            bad.append(f"sort-by {key}: order differs from sorting the printed text")
        for value in values:
            for op, keep in [("==", lambda a, b: a == b), (">", lambda a, b: a > b), ("<", lambda a, b: a < b)]:
                want = [n for n in order if keep(text[n], value)]
                got = column(directory, f'ls | where {key} {op} "{value}" | select name', "name")
                filters += 1
                if got != want:
                    bad.append(f"where {key} {op} {value!r}: {len(got)} rows, expected {len(want)}")

    print(f"checked {len(expected)} days, {0o10000} modes and {filters} where/sort-by runs")
    for line in bad[:20]:
        print(line)
    sys.exit(1 if bad else 0)
