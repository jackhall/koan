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
"""

import argparse
import re
import subprocess
import sys
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

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
