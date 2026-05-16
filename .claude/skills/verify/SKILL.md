---
name: verify
description: Use this skill to run the standard koan build-verification slate — unit tests (`cargo test`), lints (`cargo clippy --all-targets -- -D warnings`, with auto-fix of fixable issues), and the modgraph fractal-complexity score. Records the modgraph total to [`tools/complexity.txt`](../../../tools/complexity.txt) as the baseline for the next run. Invoke before pushing, before opening a PR, or whenever the user says "verify the build", "run checks", or "is this green?". Does *not* run the Miri audit slate — that has its own dedicated skill.
---

# verify

Three checks, run in order. Stop and surface on the first hard failure; the modgraph step is the final one so a regression there doesn't block tests/clippy reporting.

## 1. Unit tests

```sh
cargo test
```

Hard fail on any `FAILED`. Report `test result: ok. <N> passed; 0 failed` on success.

## 2. Lints

```sh
cargo clippy --all-targets -- -D warnings
```

If clippy reports issues, attempt to auto-fix first:

```sh
cargo clippy --fix --allow-dirty --allow-staged --all-targets
cargo clippy --all-targets -- -D warnings   # re-verify
```

If issues remain after `--fix`, fix them by hand — clippy with `-D warnings` must end clean before moving on. Per-site `#[allow(...)]` is acceptable only when the lint is genuinely wrong for the site (see [TEST.md § Linting and formatting](../../../TEST.md#linting-and-formatting) for the documented exceptions).

## 3. Modgraph fractal complexity

```sh
cargo modules dependencies --package koan --lib \
    --no-externs --no-sysroot --no-traits --no-fns --no-types \
    > /tmp/koan.dot

python3 tools/modgraph.py --edges /tmp/koan.dot --root koan \
    --baseline tools/complexity.txt
```

The `--baseline tools/complexity.txt` flag handles all the housekeeping: it reads the file, prunes stale entries (unreachable SHAs from branch checkout / hard reset / rebase drops, plus all prior dirty-snapshot `+` entries), prepends today's measurement, trims to five, and prints a one-line delta against the prior top entry. Quote that delta line verbatim in the run summary — no manual file editing required.

## End-of-run summary

A single user-facing line covering all three checks:

```
Verify: tests ok, clippy clean, modgraph per-loc <new> (Δ <signed> vs <prev>).
```

If any step hard-failed, replace the relevant clause with the failure (e.g. `tests FAILED (3 failed)`, `clippy: 2 issues remain after --fix`).

## What this skill does *not* do

- **Miri.** The audit slate is separately gated and slow; use the `miri` skill when you need memory-safety verification.
- **`cargo fmt`.** Format drift isn't gated here. Run `cargo fmt --all` separately when needed.
- **Modgraph score regressions.** A higher score is reported but doesn't fail the run — the metric is informational. Use the delta as input to a code-review judgment call.
