---
name: verify-koan
description: Use this skill to run the standard koan build-verification slate. Invoke before pushing, before opening a PR, or whenever the user says "verify the build", "run checks", or "is this green?". Does *not* run the Miri audit slate — that has its own dedicated skill.
---

# verify-koan

```sh
tools/verify.sh
```

Read [`tools/verify.sh`](../../../tools/verify.sh) for what runs and in what order.

## End-of-run summary

A single user-facing line:

```
Verify: tests ok, doctests ok, clippy clean, doclinks ok, snippets ok, coverage <pct>% (Δ <signed> vs <prev>), modgraph score <new> (Δ <signed> vs <prev>).
```

If any step hard-failed, replace the relevant clause with the failure (e.g. `tests FAILED (3 failed)`, `doctests FAILED (1 compile_fail compiled)`, `clippy: 2 issues remain after --fix`, `doclinks: 4 broken links`, `snippets FAILED (2 mismatches)`). Quote the coverage and modgraph delta lines verbatim from the script's output. If the trend log was empty (first run / no prior entry), drop the `(Δ … vs …)` suffix for that clause.

## What this skill does *not* do

- **Miri.** The audit slate is separately gated and slow; use the `miri` skill when you need memory-safety verification.
- **`cargo fmt`.** Format drift isn't gated here. Run `cargo fmt --all` separately when needed.
- **Modgraph score regressions.** A higher score is reported but doesn't fail the run — the metric is informational. Use the delta as input to a code-review judgment call.
