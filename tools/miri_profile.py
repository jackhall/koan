#!/usr/bin/env python3
"""Profile one test under Miri and say which of koan's code the interpreter's time goes to.

Miri's `-Zmiri-measureme` flag records every interpreted frame with host timestamps, so the
profile is wall time even though the test itself sees Miri's virtual clock (with isolation on,
libtest's `--report-time` counts basic blocks, not seconds). The profile is dominated by std and
dependency frames — slice indexing, iterator adapters, hashing — which say little on their own,
so this script charges each one's self time to its nearest enclosing *workspace* frame (koan's
own modules, `lattice`, `cellgraph`, `sexlex`): the koan code that asked for the work.

Usage:
  python3 tools/miri_profile.py run <test>             # profile one lib test, then report
  python3 tools/miri_profile.py report <profile>       # re-read a saved profile
  python3 tools/miri_profile.py compare <a> <b>        # what moved between two profiles
  python3 tools/miri_profile.py run <test> --flags=-Zmiri-disable-stacked-borrows   # A/B a flag
  options: --by module|function|std   --depth N (path depth, default 2; 0 = per function)   --top N
           --callers REGEX [--chain N]   (who calls the matching frames, N workspace callers up)

`run` resolves a fragment of a test's path to exactly one lib test, runs it under the slate's
flags plus measureme, and writes the profile to a fresh `scratch/miri-profiles/<test>-<time>/` (on
disk; a profile runs ~4 MB per second of interpretation). `--by std` turns the question around:
which std and dependency calls koan code makes cost the most, each charged with its whole subtree
minus any workspace code it calls back into (a closure handed to `map`), and which std modules or
functions hold the self time. `compare` lists the rows that moved most between two profiles of
one test — with and without tree borrows, say, which isolates what the aliasing model costs.

The decoder reads measureme's file format 9 directly (the `summarize` tool would need an install)
and streams the event pages, so memory stays bounded by call depth.
"""

import argparse
import collections
import re
import struct
import subprocess
import sys
import time
from pathlib import Path

from miri import MIRIFLAGS, ROOT, _env, list_lib_tests

FIRST_REGULAR_STRING_ID = 100_000_003
WORKSPACE_CRATES = {"lattice", "cellgraph", "sexlex"}
# Load phases of `CellSubstrate::load`, then the operator run, by generic-free function path.
PHASES = {
    "parse::parse_with_path": "parse",
    "dispatch::builtins::table": "builtins",
    "scope::shapes::Shapes::of_program": "shapes",
    "program::substrate::walked": "walked",
    "program::substrate::Running::run": "operator run",
    "program::substrate::CellSubstrate::load": "load (total)",
}


def koan_modules() -> set[str]:
    text = (ROOT / "src" / "lib.rs").read_text()
    mods = set(re.findall(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)", text, re.M))
    return mods - {"tests"}


class Strings:
    """The profile's string table: event ids name frames through it."""

    def __init__(self, table: bytes, index: bytes):
        self.table = table
        self.virtual = {}
        for i in range(8, len(index), 16):
            vid, addr = struct.unpack_from("<QQ", index, i)
            self.virtual[vid] = addr
        self.cache: dict[int, str] = {}

    def __getitem__(self, sid: int) -> str:
        if sid in self.cache:
            return self.cache[sid]
        p = self.virtual[sid] if sid < FIRST_REGULAR_STRING_ID else sid - FIRST_REGULAR_STRING_ID
        out = []
        while (b := self.table[p]) != 0xFF:
            if b == 0xFE:  # a reference to another string
                out.append(self[struct.unpack_from("<Q", self.table, p + 1)[0]])
                p += 9
            else:
                q = p
                while self.table[q] not in (0xFE, 0xFF):
                    q += 1
                out.append(self.table[p:q].decode("utf-8", "replace"))
                p = q
        self.cache[sid] = s = "".join(out)
        return s


def strip_generics(name: str) -> str:
    """Drop generic arguments and `::<impl …>` blocks; keep a leading `<X as Trait>`."""
    out, level = [], 0
    for i, c in enumerate(name):
        if level:
            level += {"<": 1, ">": -1}.get(c, 0)
        elif c == "<" and i and (name[i - 1].isalnum() or name[i - 1] in "_:'"):
            level = 1
        else:
            out.append(c)
    return re.sub(r"(::)+", "::", "".join(out)).rstrip(":")


def self_path(name: str) -> list[str]:
    """`<X as Trait>::f` → X's path segments; otherwise the name's own."""
    if name.startswith("<"):
        level, i = 0, 1
        while i < len(name):
            c = name[i]
            if c == "<":
                level += 1
            elif c == ">":
                if level == 0:
                    break
                level -= 1
            elif level == 0 and name.startswith(" as ", i):
                break
            i += 1
        name = name[1:i]
    return re.split(r"::<|<|\s|\{", name)[0].split("::")


class Report:
    def __init__(self, depth: int, callers: str | None = None, chain: int = 2):
        self.depth = depth
        self.callers = re.compile(callers) if callers else None
        self.chain = chain
        self.caller_time = collections.Counter()  # (target, caller chain) -> ns
        self.caller_count = collections.Counter()
        self.koan = koan_modules()
        self.owner_cache: dict[int, str | None] = {}
        self.charged = collections.Counter()  # workspace module or function -> ns
        self.phases = collections.Counter()
        self.std_self = collections.Counter()  # std frame id -> ns of self time
        self.std_calls = collections.Counter()  # std entry point called from workspace -> ns
        self.std_call_count = collections.Counter()
        self.unowned = 0

    def owner(self, sid: int) -> str | None:
        """What a workspace frame is charged to, or None for std and dependency frames."""
        if sid in self.owner_cache:
            return self.owner_cache[sid]
        name = self.names[sid]
        segments = self_path(name)
        result = None
        if segments and (segments[0] in self.koan or segments[0] in WORKSPACE_CRATES):
            if self.depth == 0:
                result = strip_generics(name).split("::{")[0]
            else:
                module = []
                for s in segments:
                    if not s or s[0].isupper() or s == "tests":
                        break
                    module.append(s)
                if "tests" in segments:
                    module = segments[: segments.index("tests") + 1]
                result = "::".join(module[: self.depth]) or segments[0]
        self.owner_cache[sid] = result
        return result

    def read(self, path: Path) -> None:
        table, index = bytearray(), bytearray()
        with open(path, "rb") as f:
            assert f.read(8)[:4] == b"MMPD", f"{path} is not a measureme profile"
            while head := f.read(5):
                tag, length = head[0], struct.unpack("<I", head[1:])[0]
                if tag == 1:
                    table += f.read(length)
                elif tag == 2:
                    index += f.read(length)
                else:
                    f.seek(length, 1)
        self.names = Strings(bytes(table), bytes(index))
        self.stream_events(path)

    def stream_events(self, path: Path) -> None:
        # Events arrive as frames finish, so a frame's callees are already on its thread's stack
        # when it arrives: (start, end, pending ns not yet charged to any workspace frame, id,
        # frames matching --callers still collecting their chain of workspace callers).
        stacks = collections.defaultdict(list)
        targets: dict[int, str | None] = {}
        waits: dict[int, bool] = {}
        phase_of: dict[int, str | None] = {}
        leftover = b""
        with open(path, "rb") as f:
            f.read(8)
            first = True
            while head := f.read(5):
                tag, length = head[0], struct.unpack("<I", head[1:])[0]
                if tag != 0:
                    f.seek(length, 1)
                    continue
                data = leftover + f.read(length)
                offset = 8 if first else 0  # the event stream opens with its own header
                first = False
                usable = offset + (len(data) - offset) // 32 * 32
                for _, eid, thread, lo1, lo2, up in struct.iter_unpack("<QQIIII", data[offset:usable]):
                    start, end = lo1 | ((up >> 16) << 32), lo2 | ((up & 0xFFFF) << 32)
                    if end == 0xFFFF_FFFF_FFFF:  # an instant event, not a frame
                        continue
                    module = self.owner(eid)
                    stack, children, pending, hits = stacks[thread], 0, 0, []
                    while stack and stack[-1][0] >= start:
                        s, e, p, child, h = stack.pop()
                        children += e - s
                        pending += p
                        hits += h
                        if module is not None and p:  # a std call made by workspace code
                            name = strip_generics(self.names[child])
                            self.std_calls[name] += p
                            self.std_call_count[name] += 1
                    own = (end - start) - children
                    if eid not in waits:  # a thread parked on a lock is not interpreting
                        waits[eid] = "futex_wait" in self.names[eid]
                    if waits[eid]:
                        own = 0
                    if hits and module is not None:
                        hits = self.extend_chains(hits, strip_generics(self.names[eid]))
                    if self.callers:
                        if eid not in targets:
                            name = strip_generics(self.names[eid])
                            targets[eid] = name if self.callers.search(name) else None
                        if targets[eid]:
                            hits.append((targets[eid], end - start, ()))
                    if module is None:
                        self.std_self[eid] += own
                        stack.append((start, end, pending + own, eid, hits))
                    else:
                        self.charged[module] += pending + own
                        stack.append((start, end, 0, eid, hits))
                    if eid not in phase_of:
                        phase_of[eid] = PHASES.get(strip_generics(self.names[eid]))
                    if phase_of[eid]:
                        self.phases[phase_of[eid]] += end - start
                leftover = data[usable:]
        self.unowned = sum(p for stack in stacks.values() for _, _, p, _, _ in stack)
        for stack in stacks.values():  # chains the profile ran out of callers for
            for *_, hits in stack:
                for target, ns, chain in hits:
                    self.caller_time[target, chain] += ns
                    self.caller_count[target, chain] += 1

    def extend_chains(self, hits: list, caller: str) -> list:
        """Add a workspace caller to each pending chain, recording the ones now long enough."""
        rest = []
        for target, ns, chain in hits:
            chain += (caller,)
            if len(chain) < self.chain:
                rest.append((target, ns, chain))
            else:
                self.caller_time[target, chain] += ns
                self.caller_count[target, chain] += 1
        return rest

    def std_group(self, sid: int) -> str:
        """A std or dependency frame's function (depth 0) or the first `depth` segments of its
        path (`core::slice`, `hashbrown::raw`)."""
        if self.depth == 0:
            return strip_generics(self.names[sid])
        m = re.search(r"(?<![\w:])[a-z_]\w*(?:::\w+)+", self.names[sid])
        return "::".join(m.group(0).split("::")[: self.depth]) if m else self.names[sid]

    def total(self) -> int:
        return sum(self.charged.values()) + self.unowned

    def table(self, by: str) -> collections.Counter:
        """Workspace charges (module, function), or std calls made by workspace code (std)."""
        return self.std_calls if by == "std" else self.charged

    def print(self, by: str, top: int) -> None:
        total = self.total()
        print(f"{total / 1e9:.1f}s interpreted ({self.unowned / 1e9:.1f}s outside any workspace frame)")
        if self.phases:
            print("phases: " + ", ".join(f"{k} {v / 1e9:.1f}s" for k, v in self.phases.most_common()))
        if by == "std":
            print("\nstd and dependency calls made by workspace code (subtree, less workspace callbacks):")
            for name, ns in self.std_calls.most_common(top):
                n = self.std_call_count[name]
                print(f"{ns / 1e9:8.2f}s {100 * ns / total:5.1f}% {n:7} calls {ns / n / 1e3:8.0f}µs  {name}")
            print(f"\nstd and dependency self time by {'function' if self.depth == 0 else 'module'}:")
            grouped = collections.Counter()
            for sid, ns in self.std_self.items():
                grouped[self.std_group(sid)] += ns
            rows = grouped.most_common(top)
        else:
            rows = self.charged.most_common(top)
        for name, ns in rows:
            print(f"{ns / 1e9:8.2f}s {100 * ns / total:5.1f}%  {name}")
        if self.callers:
            print(f"\ncallers of frames matching {self.callers.pattern!r} (nearest workspace callers first):")
            for (target, chain), ns in self.caller_time.most_common(top):
                n = self.caller_count[target, chain]
                print(f"{ns / 1e9:8.2f}s {n:6} calls  {target}  ←  " + "  ←  ".join(chain))


def compare(a: Report, b: Report, by: str, top: int) -> None:
    """The rows whose time moved most between two profiles of one test (say, with and without
    an aliasing model), largest first."""
    print(f"{a.total() / 1e9:.1f}s → {b.total() / 1e9:.1f}s interpreted")
    ta, tb = a.table(by), b.table(by)
    moved = sorted(ta.keys() | tb.keys(), key=lambda k: -abs(ta[k] - tb[k]))
    for name in moved[:top]:
        print(f"{ta[name] / 1e9:8.2f}s → {tb[name] / 1e9:6.2f}s  ({(tb[name] - ta[name]) / 1e9:+6.2f}s)  {name}")


def run(test: str, flags: str) -> Path:
    hits = [t for t in list_lib_tests([]) if test in t]
    if len(hits) != 1:
        raise SystemExit(f"{test!r} names {len(hits)} lib tests, not one: {hits[:10]}")
    full = hits[0]
    leaf = full.rsplit("::", 1)[-1]
    out = ROOT / "scratch" / "miri-profiles" / f"{leaf[:60]}-{time.strftime('%Y%m%d-%H%M%S')}"
    out.mkdir(parents=True)
    print(f"profiling {full} under Miri into {out.relative_to(ROOT)}…", file=sys.stderr)
    start = time.monotonic()
    proc = subprocess.run(
        ["cargo", "+nightly", "miri", "test", "--lib", "--quiet", "--", "--exact", full],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env={**_env(), "MIRIFLAGS": f"{MIRIFLAGS} {flags} -Zmiri-measureme={out}"},
    )
    print(f"Miri exited {proc.returncode} after {time.monotonic() - start:.0f}s", file=sys.stderr)
    if proc.returncode:
        print(proc.stdout + proc.stderr)
        raise SystemExit(1)
    # Each lib binary `--lib` runs writes its own profile; the one that ran the test is the largest.
    return max(out.glob("*.mm_profdata"), key=lambda p: p.stat().st_size)


def main() -> int:
    ap = argparse.ArgumentParser(description="Attribute a Miri profile to koan's modules.")
    ap.add_argument("verb", choices=["run", "report", "compare"])
    ap.add_argument("target", nargs="+",
                    help="a fragment naming one lib test (run), a .mm_profdata path (report), or two (compare)")
    ap.add_argument("--by", choices=["module", "function", "std"], default="module")
    ap.add_argument("--depth", type=int, default=2, help="path depth for --by module and std; 0 groups by function")
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--flags", default="", help="Miri flags added after the slate's (run)")
    ap.add_argument("--callers", metavar="REGEX",
                    help="also list who calls the frames whose name matches (time is the frame's own subtree)")
    ap.add_argument("--chain", type=int, default=2, help="workspace callers to show per --callers row")
    args = ap.parse_args()

    depth = 0 if args.by == "function" else args.depth
    if args.verb == "compare":
        a, b = Report(depth), Report(depth)
        a.read(Path(args.target[0]))
        b.read(Path(args.target[1]))
        compare(a, b, args.by, args.top)
        return 0
    profile = run(args.target[0], args.flags) if args.verb == "run" else Path(args.target[0])
    print(f"profile: {profile}")
    report = Report(depth, args.callers, args.chain)
    report.read(profile)
    report.print(args.by, args.top)
    return 0


if __name__ == "__main__":
    sys.exit(main())
