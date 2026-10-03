#!/usr/bin/env python3
"""Fail if the wire protocol changed incompatibly since a baseline release.

usage: check-protocol-compat.py BASELINE_VECTORS.json CURRENT_VECTORS.json

Compatible (additive): new vectors, i.e. appended variants or new cases.
Breaking: any baseline case missing or with different bytes, or changed
UUIDs -- unless PROTOCOL_VERSION was bumped, which declares it intentional.
"""
import json
import sys

base = json.load(open(sys.argv[1]))
cur = json.load(open(sys.argv[2]))

problems = []
if base["uuids"] != cur["uuids"]:
    problems.append(f"GATT UUIDs changed: {base['uuids']} -> {cur['uuids']}")
for kind in ("requests", "responses"):
    now = {c["name"]: c["hex"] for c in cur[kind]}
    for c in base[kind]:
        if c["name"] not in now:
            problems.append(f"{kind[:-1]} case '{c['name']}' was removed")
        elif now[c["name"]] != c["hex"]:
            problems.append(f"{kind[:-1]} case '{c['name']}' encodes differently")

if not problems:
    print(f"protocol compatible with baseline (v{base['protocol']} -> v{cur['protocol']})")
elif cur["protocol"] > base["protocol"]:
    print(f"breaking changes, but PROTOCOL_VERSION bumped {base['protocol']} -> {cur['protocol']}:")
    print("\n".join(f"  - {p}" for p in problems))
else:
    print("BREAKING wire-protocol change without a PROTOCOL_VERSION bump:")
    print("\n".join(f"  - {p}" for p in problems))
    print("Append variants instead of reordering/changing them, or bump PROTOCOL_VERSION in corisco-protocol.")
    sys.exit(1)
