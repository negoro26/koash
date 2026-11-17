#!/usr/bin/env python3
import datetime
import json
import os
import random
import subprocess
import sys
import tempfile

koa = sys.argv[1]
rng = random.Random(0)
first = datetime.date(1960, 1, 1).toordinal()
last = datetime.date(2100, 12, 31).toordinal()
far = datetime.date(2400, 12, 31).toordinal()
epoch = datetime.date(1970, 1, 1).toordinal()

with tempfile.TemporaryDirectory() as root:
    days = os.path.join(root, "days")
    modes = os.path.join(root, "modes")
    os.mkdir(days)
    os.mkdir(modes)
    expected = {}
    sampled = rng.sample(range(last + 1, far + 1), 5000)
    for day in [*range(first, last + 1), *sampled]:
        t = (day - epoch) * 86_400 + rng.randrange(86_400)
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

    def ls(directory):
        out = subprocess.run(
            [koa, "-c", "ls | select name modified permissions | to json"],
            cwd=directory, capture_output=True, text=True, check=True,
        ).stdout
        return json.loads(out)

    bad = []
    rows = ls(days)
    seen = {row["name"]: row.get("modified", "") for row in rows}
    for name, want in expected.items():
        if seen.get(name) != want:
            bad.append(f"{name}: modified {seen.get(name)!r}, expected {want!r}")
    rows = ls(modes)
    for row in rows:
        mode = int(row["name"][1:])
        want = format(mode, "o")
        if row["permissions"] != want:
            bad.append(f"{row['name']}: permissions {row['permissions']!r}, expected {want!r}")
    if len(rows) != 0o10000:
        bad.append(f"ls listed {len(rows)} of {0o10000} mode files")

    print(f"checked {len(expected)} days and {0o10000} modes")
    for line in bad[:20]:
        print(line)
    sys.exit(1 if bad else 0)
