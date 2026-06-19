#!/usr/bin/env python3
"""Verify the runnable code snippets in the tutorial (or any doc tree).

Runs every ```koan block that is immediately followed (whitespace only) by a
```text expected-output block through the interpreter and diffs the output. A
```koan block with no following ```text block is a syntax illustration, not a
runnable program, and is skipped. Output is compared line-by-line with trailing
whitespace stripped. Exits non-zero on any mismatch.

Usage (from the repo root, with the binary built — `cargo build`):
    python3 tools/verify_snippets.py                       # checks tutorial/
    python3 tools/verify_snippets.py tutorial/06-pattern-matching.md
"""
import re, subprocess, pathlib, sys

KOAN = "./target/debug/koan"
target = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "tutorial")
mds = sorted(target.glob("*.md")) if target.is_dir() else [target]

block = re.compile(r"```(\w*)\n(.*?)\n```", re.DOTALL)


def norm(s):
    return "\n".join(line.rstrip() for line in s.rstrip("\n").split("\n"))


total = fails = 0
for md in mds:
    text = md.read_text()
    ms = list(block.finditer(text))
    for i, m in enumerate(ms):
        if m.group(1) != "koan":
            continue
        if i + 1 >= len(ms) or ms[i + 1].group(1) != "text":
            continue
        if text[m.end():ms[i + 1].start()].strip() != "":
            continue  # prose between → the koan block is a fragment
        code, expected = m.group(2), ms[i + 1].group(2)
        total += 1
        res = subprocess.run([KOAN], input=code + "\n",
                             capture_output=True, text=True)
        got = res.stdout + res.stderr
        if norm(got) != norm(expected):
            fails += 1
            print(f"\n=== MISMATCH in {md.name} ===")
            print("--- CODE ---\n" + code)
            print("--- EXPECTED ---\n" + expected)
            print("--- GOT ---\n" + got.rstrip())
print(f"\n{total - fails}/{total} runnable snippets matched; {fails} mismatches")
sys.exit(1 if fails else 0)
