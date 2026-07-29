---
name: verify-koan
description: Use this skill to run the standard koan build-verification slate. Invoke before pushing, before opening a PR, or whenever the user says "verify the build", "run checks", or "is this green?". Does *not* run the Miri audit slate — that has its own dedicated skill.
---

# verify-koan

```sh
tools/verify.sh
```

Read [`tools/verify.sh`](../../../tools/verify.sh) for what runs and in what order.

## Two slates, picked by change scope

The script inspects every path differing from `HEAD` (staged, unstaged, and untracked) and picks one of two slates. Nothing to configure — just report which one ran.

- **Full slate** (8 steps) whenever any changed path sits outside `workgraph/`, and on a clean tree. koan compiling is a gate, as are coverage and the modgraph score.
- **Library slate** (3 steps) when *every* changed path is under `workgraph/`: `cargo test -p workgraph` (unit tests and doctests, `--features test-hooks`), clippy on the same, and doclinks. It then reports whether koan still compiles **as information, never as a gate.**

The library slate exists so a workgraph change can land ahead of koan's adoption of it — see [the library roadmap's convention](../../../workgraph/roadmap/README.md). koan failing to compile against workgraph `HEAD` is the expected mid-migration state there, so treat the reported error count as the size of the debt now owed, not as a failure to fix before committing. Coverage, snippets, and the modgraph score are koan-rooted and do not run; the trend logs are not rebaselined.

## End-of-run summary

A single user-facing line. Full slate:

```
Verify: tests ok, doctests ok, clippy clean, doclinks ok, snippets ok, coverage <pct>% (Δ <signed> vs <prev>), modgraph score <new> (Δ <signed> vs <prev>).
```

Library slate — always name the scope, so it is never mistaken for a full run:

```
Verify (workgraph only): tests ok, clippy clean, doclinks ok; koan <compiles | does NOT compile — N errors of adoption debt>.
```

If any step hard-failed, replace the relevant clause with the failure (e.g. `tests FAILED (3 failed)`, `doctests FAILED (1 compile_fail compiled)`, `clippy: 2 issues remain after --fix`, `doclinks: 4 broken links`, `snippets FAILED (2 mismatches)`). Quote the coverage and modgraph delta lines verbatim from the script's output. If the trend log was empty (first run / no prior entry), drop the `(Δ … vs …)` suffix for that clause. The koan clause of the library-slate line is a status report, not a failure — never phrase it as one.

## What this skill does *not* do

- **Miri.** The audit slate is separately gated and slow; use the `miri` skill when you need memory-safety verification.
- **`cargo fmt`.** Format drift isn't gated here. Run `cargo fmt --all` separately when needed.
- **Modgraph score regressions.** A higher score is reported but doesn't fail the run. Use the delta as input to a code-review judgment call, and report it to the user.
