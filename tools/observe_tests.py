#!/usr/bin/env python3
"""Observe the koan test suite. Subcommands:

  list   — classify every `#[test]` in src/ as koan-language or rust-language
           and tag what it exercises. Tab-separated rows:
               PATH  LINE  KIND  NAME  BODY_LOC  TAGS

           A test is koan-language if its body calls one of the koan-source
           entry points (`parse`, `parse_one`, `run`, `run_one`, `run_one_err`,
           `run_root_silent`, `run_root_bare`, `interpret`,
           `interpret_with_writer`); otherwise rust-language.

           For koan tests, TAGS lists language features detected in the
           string-literal args (keywords LET/FN/STRUCT/UNION/MODULE/SIG/VAL/
           MATCH/PRINT/IF/TYPE_CONSTRUCTOR; sigils :|, :!, ->).

           For rust tests, TAGS lists the set of `Type::fn(` and `.method(`
           calls found in the body — filterable downstream by grep.

           With `--matrix PATH`, also parses backtick-quoted test names (or
           `file.rs::name` refs) in the matrix file and emits STALE diagnostics
           for any that don't resolve to a discovered test.

  slate  — emit the Miri audit-slate test names from observe/miri_slate.md,
           space-separated on a single line for direct interpolation into the
           miri command of record:

               MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test \\
                   --quiet -- $(python3 tools/observe_tests.py slate)

  slate-audit
         — diff `src/` `unsafe` sites against the slate's group anchors and
           the cached `<!-- slate-fingerprint -->` block. Exits non-zero on
           drift. `--update` rewrites the fingerprint block.

  audit  — run `slate-audit` and then `cargo llvm-cov` test-coverage; writes
           the lcov file to `observe/coverage.lcov` (override with
           `--coverage`). Exits non-zero if either gate fails.

  overlap
         — per-test coverage overlap analysis. Runs each rust-language
           `#[test]` (koan-language tests are excluded — their Koan
           source strings are part of the spec, so coverage similarity
           does not imply they are redundant) under llvm source-based
           coverage with a unique profraw, then reports within-module
           Jaccard-overlap pairs and strict subsets.

           Two-phase workflow:

               # one-time run (~10-15 min): collect raw per-test coverage
               python3 tools/observe_tests.py overlap \\
                   --raw observe/test_overlap_raw.json

               # cheap re-analysis with different thresholds
               python3 tools/observe_tests.py overlap \\
                   --from-raw observe/test_overlap_raw.json \\
                   --min-jaccard 0.7
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path

ENTRY_POINTS = {
    "parse", "parse_one",
    "run", "run_one", "run_one_err",
    "run_root_silent", "run_root_bare",
    "interpret", "interpret_with_writer",
}

KEYWORDS = [
    "LET", "FN", "STRUCT", "UNION", "MODULE", "SIG", "VAL",
    "MATCH", "PRINT", "IF", "TYPE_CONSTRUCTOR",
]
SIGILS = [":|", ":!", "->"]


def blank_strings_and_comments(text: str) -> str:
    """Replace string-literal bodies and comment bodies with spaces, preserving
    newlines and overall length. Used for brace-matching the test body so that
    a `{` or `}` inside a koan-source string doesn't throw off the scan."""
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        # raw string r"..." or r#..#"..."#..#
        if c == 'r' and i + 1 < n and text[i + 1] in ('"', '#'):
            j = i + 1
            hashes = 0
            while j < n and text[j] == '#':
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                end_marker = '"' + '#' * hashes
                k = text.find(end_marker, j + 1)
                k = n if k < 0 else k + len(end_marker)
                for x in range(i, min(k, n)):
                    if out[x] != '\n':
                        out[x] = ' '
                i = k
                continue
        # normal or byte string
        if c == '"' or (c == 'b' and i + 1 < n and text[i + 1] == '"'):
            if c == 'b':
                out[i] = ' '
                i += 1
            j = i + 1
            out[i] = ' '
            while j < n:
                if text[j] == '\\' and j + 1 < n:
                    for x in (j, j + 1):
                        if out[x] != '\n':
                            out[x] = ' '
                    j += 2
                elif text[j] == '"':
                    out[j] = ' '
                    j += 1
                    break
                else:
                    if out[j] != '\n':
                        out[j] = ' '
                    j += 1
            i = j
            continue
        # line comment
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            while i < n and text[i] != '\n':
                out[i] = ' '
                i += 1
            continue
        # block comment (nestable per Rust)
        if c == '/' and i + 1 < n and text[i + 1] == '*':
            out[i] = ' '
            out[i + 1] = ' '
            i += 2
            depth = 1
            while i < n and depth > 0:
                if text[i] == '/' and i + 1 < n and text[i + 1] == '*':
                    depth += 1
                    out[i] = ' '
                    out[i + 1] = ' '
                    i += 2
                elif text[i] == '*' and i + 1 < n and text[i + 1] == '/':
                    depth -= 1
                    out[i] = ' '
                    out[i + 1] = ' '
                    i += 2
                else:
                    if out[i] != '\n':
                        out[i] = ' '
                    i += 1
            continue
        # char literal or lifetime — skip the single quote (either way nothing
        # inside it should be a `{` or `}` brace).
        if c == "'":
            i += 1
            continue
        i += 1
    return ''.join(out)


TEST_RE = re.compile(
    r"#\[test\][^\n]*\n(?:\s*#\[[^\n]*\]\s*\n)*\s*(?:async\s+)?fn\s+(?P<name>[A-Za-z_][A-Za-z_0-9]*)\s*\(",
)

CALL_RE = re.compile(r"(?<![A-Za-z_0-9:])([a-z_][A-Za-z_0-9]*)\s*\(")
SUBJECT_RE = re.compile(
    r"([A-Z][A-Za-z_0-9]*(?:::[A-Za-z_][A-Za-z_0-9]*)+)\s*\("  # Type::fn(
    r"|\.([a-z_][A-Za-z_0-9]*)\s*\("                          # .method(
)
STRING_RE = re.compile(
    r'r#*"(?P<raw>(?:[^"]|"(?!#*))*?)"#*'                     # raw strings — coarse
    r"|"
    r'"(?P<norm>(?:\\.|[^"\\])*)"',                           # normal strings
    re.DOTALL,                                                # so `\<newline>` continuations match
)


def iter_tests(path: Path):
    text = path.read_text()
    blanked = blank_strings_and_comments(text)
    for m in TEST_RE.finditer(text):
        name = m.group("name")
        # Skip past the parameter list `()` in the blanked view.
        i = m.end()
        depth = 1
        while i < len(blanked) and depth > 0:
            if blanked[i] == '(':
                depth += 1
            elif blanked[i] == ')':
                depth -= 1
            i += 1
        # Walk to the opening `{` of the body.
        while i < len(blanked) and blanked[i] != '{':
            i += 1
        if i >= len(blanked):
            continue
        open_brace = i
        depth = 1
        j = i + 1
        while j < len(blanked) and depth > 0:
            if blanked[j] == '{':
                depth += 1
            elif blanked[j] == '}':
                depth -= 1
            j += 1
        body = text[open_brace + 1:j - 1]
        line = text.count('\n', 0, m.start()) + 1
        yield line, name, body


def collect_features(body: str):
    feats = set()
    # Match all string literals in the body (raw + normal), look for keywords/sigils.
    for sm in STRING_RE.finditer(body):
        content = sm.group("raw") if sm.group("raw") is not None else sm.group("norm")
        if content is None:
            continue
        for kw in KEYWORDS:
            if re.search(r"\b" + re.escape(kw) + r"\b", content):
                feats.add(kw)
        for sig in SIGILS:
            if sig in content:
                feats.add(sig)
    return sorted(feats)


def collect_subjects(blanked_body: str):
    subj = set()
    for m in SUBJECT_RE.finditer(blanked_body):
        if m.group(1):
            subj.add(m.group(1))
        elif m.group(2):
            subj.add("." + m.group(2))
    return sorted(subj)


def classify(body: str):
    blanked = blank_strings_and_comments(body)
    for m in CALL_RE.finditer(blanked):
        if m.group(1) in ENTRY_POINTS:
            return "koan", collect_features(body)
    return "rust", collect_subjects(blanked)


def check_matrix(matrix_path: Path, all_tests):
    text = matrix_path.read_text()
    refs = set()
    for m in re.finditer(r"`([^`]+)`", text):
        token = m.group(1).strip()
        if "::" in token:
            refs.add(token)
        elif re.match(r"^[a-z_][a-z_0-9]*$", token):
            refs.add(token)
    names = {n for _, n in all_tests}
    paths_names = {f"{p}::{n}" for p, n in all_tests}
    stale = []
    for r in refs:
        if "::" in r:
            if r not in paths_names and r.split("::")[-1] not in names:
                stale.append(r)
        elif r not in names:
            stale.append(r)
    if stale:
        print(f"\nSTALE references in {matrix_path}:", file=sys.stderr)
        for s in sorted(stale):
            print(f"  {s}", file=sys.stderr)
        return 1
    return 0


SLATE_TEST_RE = re.compile(r"^- `([A-Za-z_][A-Za-z_0-9]*)`\s*$", re.MULTILINE)

# Captures the display path inside the group-header pattern:
#     **Group name** ([src/path/to/file.rs](...))
SLATE_GROUP_RE = re.compile(r"\*\*[^*]+\*\*\s*\(\[(src/[^\]]+\.rs)\]")
# Visible whitelist for the stale-group check: paths listed (one per `- \`path\``
# bullet) between the start/end sentinels are exempted when they have no
# `unsafe` left, on the rationale that the test anchors a safe-code invariant
# (e.g. a `RefCell` discipline) rather than an `unsafe` site. The block lives
# under the `## Stale-group whitelist` heading near the top of the slate.
SLATE_WHITELIST_RE = re.compile(
    r"<!-- slate-audit-whitelist:start -->(.*?)<!-- slate-audit-whitelist:end -->",
    re.DOTALL,
)
SLATE_WHITELIST_PATH_RE = re.compile(r"^- `(src/[^`]+\.rs)`", re.MULTILINE)

UNSAFE_RE = re.compile(r"\bunsafe\b")

FINGERPRINT_RE = re.compile(
    r"<!--\s*slate-fingerprint\s*\n(.*?)\n-->",
    re.DOTALL,
)


def count_unsafe(src_root: Path):
    """Return {path-str: count} for every src file whose code carries `unsafe`."""
    counts = {}
    for path in sorted(src_root.rglob("*.rs")):
        text = path.read_text()
        blanked = blank_strings_and_comments(text)
        n = len(UNSAFE_RE.findall(blanked))
        if n > 0:
            counts[str(path)] = n
    return counts


def parse_slate_groups(slate_text: str):
    """Return the set of source-file paths referenced as slate group anchors."""
    return set(SLATE_GROUP_RE.findall(slate_text))


def parse_safe_anchors(slate_text: str):
    """Return the set of anchor paths whitelisted from the stale-group check."""
    m = SLATE_WHITELIST_RE.search(slate_text)
    if not m:
        return set()
    return set(SLATE_WHITELIST_PATH_RE.findall(m.group(1)))


def parse_fingerprint(slate_text: str):
    """Return {path: count} from the fingerprint block, or {} if absent."""
    m = FINGERPRINT_RE.search(slate_text)
    if not m:
        return {}
    out = {}
    for line in m.group(1).splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if ":" not in line:
            continue
        path, _, count = line.rpartition(":")
        try:
            out[path.strip()] = int(count.strip())
        except ValueError:
            continue
    return out


def render_fingerprint(counts):
    body = "\n".join(f"{p}: {n}" for p, n in sorted(counts.items()))
    return f"<!-- slate-fingerprint\n{body}\n-->"


def cmd_slate_audit(args):
    slate_path = Path(args.slate)
    if not slate_path.exists():
        print(f"slate file not found: {slate_path}", file=sys.stderr)
        sys.exit(1)
    slate_text = slate_path.read_text()
    src_root = Path(args.src)

    live_counts = count_unsafe(src_root)
    slate_files = parse_slate_groups(slate_text)
    safe_anchors = parse_safe_anchors(slate_text)
    fingerprints = parse_fingerprint(slate_text)

    live_files = set(live_counts.keys())
    new_unsafe = sorted(live_files - slate_files)
    stale_groups = sorted((slate_files - live_files) - safe_anchors)
    changed_counts = sorted(
        (p, fingerprints[p], live_counts[p])
        for p in live_files & set(fingerprints.keys())
        if fingerprints[p] != live_counts[p]
    )
    missing_fingerprint = sorted(live_files - set(fingerprints.keys()))

    if args.update:
        new_text = render_fingerprint(live_counts)
        if FINGERPRINT_RE.search(slate_text):
            slate_text = FINGERPRINT_RE.sub(new_text, slate_text)
        else:
            # Insert after the first level-1 heading + its paragraph blank line.
            lines = slate_text.splitlines(keepends=True)
            insert_at = 0
            for i, ln in enumerate(lines):
                if ln.startswith("# "):
                    # walk to the next blank line
                    for j in range(i + 1, len(lines)):
                        if lines[j].strip() == "":
                            insert_at = j + 1
                            break
                    break
            lines.insert(insert_at, new_text + "\n\n")
            slate_text = "".join(lines)
        slate_path.write_text(slate_text)
        print(f"updated fingerprint block in {slate_path} ({len(live_counts)} files)")
        return

    print(f"## unsafe site coverage ({slate_path})")
    if new_unsafe:
        print("\nfiles with `unsafe` but NO slate group (potentially new uncovered sites):")
        for p in new_unsafe:
            print(f"  + {p}  (unsafe count: {live_counts[p]})")
    if stale_groups:
        print("\nfiles in slate but NO `unsafe` left (potentially stale group):")
        for p in stale_groups:
            print(f"  - {p}")
    if changed_counts:
        print("\nunsafe-count changed since last fingerprint (review whether new sites are covered):")
        for p, before, after in changed_counts:
            print(f"  ~ {p}: {before} -> {after}")
    if missing_fingerprint:
        print("\nfiles with `unsafe` but no fingerprint entry (run --update to record):")
        for p in missing_fingerprint:
            print(f"  ? {p}  (unsafe count: {live_counts[p]})")
    if not (new_unsafe or stale_groups or changed_counts or missing_fingerprint):
        print("\nslate is in sync with src/ unsafe sites.")
    # Limits / caveats
    print("\nlimits: file-level granularity. A slate test can pin behavior in a")
    print("file other than its own — group-header paths are the anchor.")

    if new_unsafe or stale_groups or changed_counts:
        sys.exit(1)


def cmd_list(args):
    src_root = Path(args.src)
    rows = []
    all_tests = []
    for path in sorted(src_root.rglob("*.rs")):
        for line, name, body in iter_tests(path):
            kind, tags = classify(body)
            body_loc = body.count('\n')
            rows.append((str(path), line, kind, name, body_loc, ",".join(tags) if tags else "-"))
            all_tests.append((str(path), name))

    print("PATH\tLINE\tKIND\tNAME\tBODY_LOC\tTAGS")
    for r in rows:
        print("\t".join(str(x) for x in r))

    if args.matrix:
        rc = check_matrix(Path(args.matrix), all_tests)
        sys.exit(rc)


def cmd_slate(args):
    slate_path = Path(args.slate)
    if not slate_path.exists():
        print(f"slate file not found: {slate_path}", file=sys.stderr)
        sys.exit(1)
    names = SLATE_TEST_RE.findall(slate_path.read_text())
    if not names:
        print(f"no test names parsed from {slate_path}", file=sys.stderr)
        sys.exit(1)
    print(" ".join(names))


def cmd_audit(args):
    """Run slate-audit, then `cargo llvm-cov` for test-coverage. Writes the lcov
    file to `observe/coverage.lcov`; prints a per-file summary to stdout. Exits
    non-zero if either gate fails."""
    print("# slate-audit", flush=True)
    slate_proc = subprocess.run(
        [sys.executable, __file__, "slate-audit", "--slate", args.slate, "--src", args.src],
        check=False,
    )
    cov_path = Path(args.coverage)
    cov_path.parent.mkdir(parents=True, exist_ok=True)
    print(f"\n# test-coverage ({cov_path})", flush=True)
    cov_proc = subprocess.run(
        ["cargo", "llvm-cov", "--quiet", "--lcov", "--output-path", str(cov_path)],
        check=False,
    )
    sys.exit(slate_proc.returncode or cov_proc.returncode)


# ---------- overlap subcommand --------------------------------------------
#
# Per-test coverage Jaccard analysis. Re-uses `iter_tests` / `classify` to
# skip koan-language tests (whose Koan source strings are part of the spec,
# not redundant scaffolding). Operates on the rust-language subset only.

ROOT = Path(__file__).resolve().parent.parent


def _llvm_bin_dir() -> Path:
    sysroot = subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()
    for triple in (Path(sysroot) / "lib" / "rustlib").iterdir():
        cand = triple / "bin" / "llvm-profdata"
        if cand.exists():
            return cand.parent
    sys.exit("llvm-tools-preview not installed (rustup component add llvm-tools-preview)")


def _build_instrumented() -> list[Path]:
    """Build (and incidentally run once) the instrumented test binaries."""
    proc = subprocess.run(
        ["cargo", "llvm-cov", "--no-report", "--tests"],
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        sys.exit(proc.returncode)
    deps = ROOT / "target" / "llvm-cov-target" / "debug" / "deps"
    bins: list[Path] = []
    for p in deps.iterdir():
        if p.is_file() and "." not in p.name and os.access(p, os.X_OK):
            bins.append(p)
    _purge_profraws()
    return bins


def _purge_profraws() -> None:
    """Sweep stale profraws from the workspace root and target/. Build steps and
    stray instrumented-binary invocations both deposit them; clean them up so
    they don't accumulate."""
    for d in (ROOT, ROOT / "target", ROOT / "target" / "llvm-cov-target"):
        if d.exists():
            for raw in d.glob("*.profraw"):
                raw.unlink()


def _list_binary_tests(binary: Path, sink_profraw: Path) -> list[str]:
    """List tests in an instrumented binary. Pin LLVM_PROFILE_FILE to a
    throwaway path inside the staging dir — instrumented binaries flush a
    profraw on every exit, including a `--list` invocation. Without this, the
    flush lands in CWD as `default_*.profraw` and litters the workspace."""
    e = os.environ.copy()
    e["LLVM_PROFILE_FILE"] = str(sink_profraw)
    r = subprocess.run(
        [str(binary), "--list", "--format=terse"],
        capture_output=True, text=True, env=e,
    )
    return [l[: -len(": test")] for l in r.stdout.splitlines() if l.endswith(": test")]


def _run_one(binary: Path, name: str, profraw: Path) -> None:
    e = os.environ.copy()
    e["LLVM_PROFILE_FILE"] = str(profraw)
    subprocess.run(
        [str(binary), name, "--exact", "--quiet"],
        env=e, capture_output=True, timeout=120, check=False,
    )


def _export_lcov(llvm: Path, binaries: list[Path], profraw: Path, profdata: Path) -> str:
    subprocess.run(
        [str(llvm / "llvm-profdata"), "merge", "-sparse", str(profraw), "-o", str(profdata)],
        check=True, capture_output=True,
    )
    cmd = [str(llvm / "llvm-cov"), "export", "-format=lcov", f"-instr-profile={profdata}", str(binaries[0])]
    for b in binaries[1:]:
        cmd += ["-object", str(b)]
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def _parse_lcov_hits(lcov: str) -> dict[str, list[int]]:
    """{relative_file: [hit_line, ...]} restricted to workspace src/ files."""
    by_file: dict[str, list[int]] = {}
    src: str | None = None
    root = str(ROOT) + "/"
    for line in lcov.splitlines():
        if line.startswith("SF:"):
            path = line[3:]
            if path.startswith(root) and "/target/" not in path:
                src = path[len(root):]
                by_file.setdefault(src, [])
            else:
                src = None
        elif src and line.startswith("DA:"):
            ln_s, hit_s = line[3:].split(",", 1)
            if int(hit_s) > 0:
                by_file[src].append(int(ln_s))
    return {k: v for k, v in by_file.items() if v}


def _owning_module(qualified: str) -> str | None:
    """Path under src/ that the test exercises (drop test name + test mod + crate)."""
    parts = qualified.split("::")
    if len(parts) < 2:
        return None
    parts = parts[:-1]
    for i in range(len(parts) - 1, -1, -1):
        if parts[i] == "tests" or parts[i].endswith("_tests"):
            parts = parts[:i]
            break
    if len(parts) < 2:
        return None
    return "/".join(parts[1:])


def _module_files(module: str) -> set[str]:
    if not module:
        return set()
    files: set[str] = set()
    flat = ROOT / "src" / (module + ".rs")
    if flat.exists():
        files.add(f"src/{module}.rs")
    nested = ROOT / "src" / module
    if nested.is_dir():
        for p in nested.rglob("*.rs"):
            files.add(str(p.relative_to(ROOT)))
    return files


def _save_raw(path: Path, per_test: dict[str, dict[str, list[int]]]) -> None:
    files: list[str] = sorted({f for cov in per_test.values() for f in cov})
    idx = {f: i for i, f in enumerate(files)}
    payload = {
        "files": files,
        "tests": {t: {str(idx[f]): sorted(lns) for f, lns in cov.items()} for t, cov in per_test.items()},
    }
    path.write_text(json.dumps(payload))


def _load_raw(path: Path) -> dict[str, dict[str, list[int]]]:
    payload = json.loads(path.read_text())
    files = payload["files"]
    return {t: {files[int(i)]: lns for i, lns in cov.items()} for t, cov in payload["tests"].items()}


def _collect_coverage(filter_re: re.Pattern | None) -> dict[str, dict[str, list[int]]]:
    llvm = _llvm_bin_dir()
    print("# building instrumented test binaries", file=sys.stderr)
    binaries = _build_instrumented()
    print(f"#   found {len(binaries)} test binaries", file=sys.stderr)

    staging = ROOT / "observe" / ".profraw-staging"
    staging.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="overlap-", dir=staging))
    profraw = work / "p.profraw"
    profdata = work / "p.profdata"
    sink = work / "sink.profraw"

    per_test: dict[str, dict[str, list[int]]] = {}
    try:
        tally = 0
        for binary in binaries:
            for t in _list_binary_tests(binary, sink):
                qualified = f"{binary.stem.rsplit('-', 1)[0]}::{t}"
                if filter_re and not filter_re.search(qualified):
                    continue
                tally += 1
                if profraw.exists():
                    profraw.unlink()
                _run_one(binary, t, profraw)
                if not profraw.exists():
                    per_test[qualified] = {}
                    continue
                lcov = _export_lcov(llvm, binaries, profraw, profdata)
                per_test[qualified] = _parse_lcov_hits(lcov)
                if tally % 25 == 0:
                    print(f"#   processed {tally} tests", file=sys.stderr)
    finally:
        shutil.rmtree(work, ignore_errors=True)
        _purge_profraws()
        # Drop the staging parent if empty (no concurrent runs).
        try:
            staging.rmdir()
        except OSError:
            pass
    return per_test


def _kind_by_name(src_root: Path) -> dict[str, str]:
    """Classify every #[test] by simple name for overlap exclusion.

    Strengthens the base `classify()` (which only catches direct entry-point
    calls) by also tagging as koan any test whose string literals contain
    Koan keywords/sigils. This catches wrapper-delegated cases like
    `capture_program_output(source)` → `run(scope, source)` and
    `parse_all(src)` → `parse(src)`, where the entry-point call is hidden
    inside a crate-local helper.

    Every test under `src/parse/**` is unconditionally koan: parse tests
    drive token/whitespace-level Koan inputs that don't contain the
    high-level keywords the string-scan looks for, but each input is still
    spec.

    On simple-name collisions across files, koan wins (conservative — we'd
    rather skip a candidate than collapse a Koan-input-spec test).
    """
    out: dict[str, str] = {}
    parse_root = src_root / "parse"
    for path in sorted(src_root.rglob("*.rs")):
        in_parse = parse_root in path.parents or path == src_root / "parse.rs"
        for _line, name, body in iter_tests(path):
            if in_parse:
                kind = "koan"
            else:
                kind, _ = classify(body)
                if kind == "rust" and collect_features(body):
                    kind = "koan"
            if name not in out or out[name] == "rust":
                out[name] = kind
    return out


def cmd_overlap(args):
    if args.from_raw:
        print(f"# loading raw coverage from {args.from_raw}", file=sys.stderr)
        per_test = _load_raw(Path(args.from_raw))
        if args.filter:
            r = re.compile(args.filter)
            per_test = {n: c for n, c in per_test.items() if r.search(n)}
    else:
        filter_re = re.compile(args.filter) if args.filter else None
        per_test = _collect_coverage(filter_re)
        if args.raw:
            raw_out = Path(args.raw)
            raw_out.parent.mkdir(parents=True, exist_ok=True)
            _save_raw(raw_out, per_test)
            print(f"# wrote {raw_out}", file=sys.stderr)

    kind_by_name = _kind_by_name(Path(args.src))
    rust_only = {
        t: cov for t, cov in per_test.items()
        if kind_by_name.get(t.rsplit("::", 1)[-1]) == "rust"
    }
    excluded_koan = len(per_test) - len(rust_only)

    module_files_cache: dict[str, set[str]] = {}
    groups: dict[str | None, list[tuple[str, frozenset[tuple[str, int]]]]] = defaultdict(list)
    empty: list[str] = []
    for name, cov in rust_only.items():
        mod = _owning_module(name)
        if mod is None:
            continue
        if mod not in module_files_cache:
            module_files_cache[mod] = _module_files(mod)
        restrict = module_files_cache[mod]
        hits = frozenset((f, ln) for f, lns in cov.items() if f in restrict for ln in lns)
        if not hits:
            empty.append(name)
            continue
        groups[mod].append((name, hits))

    subsets: list[tuple[str, str, int, int, str]] = []
    near_dup: list[tuple[str, str, float, int, str]] = []
    for mod, items in groups.items():
        items.sort(key=lambda kv: len(kv[1]))
        for i, (a, sa) in enumerate(items):
            for b, sb in items[i + 1:]:
                if sa <= sb and len(sa) < len(sb):
                    subsets.append((a, b, len(sa), len(sb), mod))
                inter = len(sa & sb)
                if inter == 0:
                    continue
                union = len(sa | sb)
                j = inter / union
                if j >= args.min_jaccard and a != b:
                    near_dup.append((a, b, j, union, mod))
    subsets.sort(key=lambda r: (r[3] - r[2], r[2]))
    near_dup.sort(key=lambda r: -r[2])

    lines: list[str] = [
        "# test overlap report",
        "",
        "mode: rust-only, within-module",
        f"tests analyzed: {len(rust_only)}  (koan-excluded: {excluded_koan}; empty-under-restriction: {len(empty)})",
        f"jaccard threshold: {args.min_jaccard}",
        "",
        "## empty coverage under restriction",
        "",
    ]
    if empty:
        lines.append("Rust-language tests whose owning-module coverage is empty (consider relocating):")
        lines.append("")
        for n in empty[: args.limit]:
            lines.append(f"- `{n}`")
    else:
        lines.append("_none_")
    lines += ["", "## subset tests (A's lines ⊆ B's lines)", "",
              "| module | A (subset) | |A| | B (superset) | |B| |", "|---|---|---:|---|---:|"]
    for a, b, sa, sb, mod in subsets[: args.limit]:
        lines.append(f"| `{mod}` | `{a}` | {sa} | `{b}` | {sb} |")
    if not subsets:
        lines.append("| _none_ | | | | |")
    lines += ["", f"## near-duplicate pairs (jaccard ≥ {args.min_jaccard})", "",
              "| module | A | B | jaccard | union |", "|---|---|---|---:|---:|"]
    for a, b, j, u, mod in near_dup[: args.limit]:
        lines.append(f"| `{mod}` | `{a.rsplit('::', 1)[-1]}` | `{b.rsplit('::', 1)[-1]}` | {j:.3f} | {u} |")
    if not near_dup:
        lines.append("| _none_ | | | | |")

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(lines) + "\n")
    print(f"# wrote {out_path}", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    ap_list = sub.add_parser("list", help="classify every #[test] in src/ as koan- or rust-language")
    ap_list.add_argument("--src", default="src", help="source root (default: src)")
    ap_list.add_argument("--matrix", default=None, help="coverage-matrix file to check for stale refs")
    ap_list.set_defaults(func=cmd_list)

    ap_slate = sub.add_parser("slate", help="emit miri audit-slate test names space-separated")
    ap_slate.add_argument("--slate", default="observe/miri_slate.md", help="slate markdown file (default: observe/miri_slate.md)")
    ap_slate.set_defaults(func=cmd_slate)

    ap_slate_audit = sub.add_parser("slate-audit", help="diff src/ unsafe sites against slate coverage + fingerprint counts")
    ap_slate_audit.add_argument("--slate", default="observe/miri_slate.md", help="slate markdown file (default: observe/miri_slate.md)")
    ap_slate_audit.add_argument("--src", default="src", help="source root (default: src)")
    ap_slate_audit.add_argument("--update", action="store_true", help="rewrite the slate's <!-- slate-fingerprint --> block to match the current live counts")
    ap_slate_audit.set_defaults(func=cmd_slate_audit)

    ap_audit = sub.add_parser("audit", help="run slate-audit + `cargo llvm-cov` test-coverage; write lcov to observe/")
    ap_audit.add_argument("--slate", default="observe/miri_slate.md", help="slate markdown file (default: observe/miri_slate.md)")
    ap_audit.add_argument("--src", default="src", help="source root (default: src)")
    ap_audit.add_argument("--coverage", default="observe/coverage.lcov", help="lcov output path (default: observe/coverage.lcov)")
    ap_audit.set_defaults(func=cmd_audit)

    ap_overlap = sub.add_parser("overlap", help="per-test coverage Jaccard analysis (rust-language tests only)")
    ap_overlap.add_argument("--src", default="src", help="source root (default: src)")
    ap_overlap.add_argument("--filter", default=None, help="regex on fully-qualified test name")
    ap_overlap.add_argument("--min-jaccard", type=float, default=0.9)
    ap_overlap.add_argument("--limit", type=int, default=100, help="max rows per report section")
    ap_overlap.add_argument("--out", default="observe/test_overlap.md", help="report output (default: observe/test_overlap.md)")
    ap_overlap.add_argument("--raw", default="observe/test_overlap_raw.json", help="save raw per-test coverage")
    ap_overlap.add_argument("--from-raw", default=None, help="skip running tests; load raw coverage from this path")
    ap_overlap.set_defaults(func=cmd_overlap)

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
