#!/usr/bin/env python3
import json
import os
import random
import subprocess
import sys
import tempfile

koa = os.path.abspath(sys.argv[1])
count = int(sys.argv[2]) if len(sys.argv) > 2 else 3000
rng = random.Random(1)
KEYS = ["a", "b", "z", "é", "key with space", "n", "x1", "_"]
STRINGS = ["", "x", "é漢字", "l1\nl2", "tab\there", 'q"uote', "back\\slash", "{not json}", "1", "null"]
NUMBERS = [0, 1, -1, 2**53, 2**64 - 1, -(2**63), 0.5, -2.25, 1e21, 1e300, 2.0]
EDGES = [
    '{"a":1,"a":2}',
    '{"a":null,"b":null}',
    '{"n":[1,null,{"k":null}],"o":{"x":null,"y":[]}}',
    '{"s":"\\n","t":"é","u":""}',
    '{"big":18446744073709551615,"neg":-9223372036854775808,"f":1e21}',
    '{"z":1,"a":2,"m":3}',
]


def scalar():
    pick = rng.randrange(5)
    if pick == 0:
        return None
    if pick == 1:
        return rng.choice([True, False])
    if pick == 2:
        return rng.choice(NUMBERS)
    return rng.choice(STRINGS)


def value(depth):
    if depth > 2 or rng.random() < 0.6:
        return scalar()
    if rng.random() < 0.5:
        return [value(depth + 1) for _ in range(rng.randrange(4))]
    return {rng.choice(KEYS): value(depth + 1) for _ in range(rng.randrange(4))}


generated = [{rng.choice(KEYS): value(0) for _ in range(rng.randrange(1, 6))} for _ in range(count)]
lines = EDGES + [json.dumps(r, ensure_ascii=False) for r in generated]
records = [json.loads(line) for line in lines]
expected = [{k: v for k, v in r.items() if v is not None} for r in records]

with tempfile.TemporaryDirectory() as root:
    with open(os.path.join(root, "in.json"), "w") as f:
        f.write("\n".join(lines) + "\n")
    out = subprocess.run(
        [koa, "-c", "cat --raw in.json | from json | to json"],
        cwd=root, capture_output=True, text=True,
    )
    if out.returncode != 0:
        print(out.stderr)
        sys.exit(1)
    got = json.loads(out.stdout)

bad = [i for i, (g, e) in enumerate(zip(got, expected)) if g != e]
print(f"checked {len(lines)} records ({len(EDGES)} fixed edge cases), {len(got)} came back, {len(bad)} differ")
for i in bad[:5]:
    print(f"record {i}\n  sent {json.dumps(records[i], ensure_ascii=False)}\n  got  {json.dumps(got[i], ensure_ascii=False)}")
sys.exit(1 if bad or len(got) != len(lines) else 0)
