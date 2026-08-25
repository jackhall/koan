---
name: verify-koan
description: Use this skill to run the standard koan build-verification slate. Invoke before pushing, before opening a PR, or whenever the user says "verify the build", "run checks", or "is this green?". Does *not* run the Miri audit slate — that has its own dedicated skill.
---

# verify-koan

```sh
tools/verify.sh
```

Read [`tools/verify.sh`](../../../tools/verify.sh) for what runs and in what order.

**One invocation is the whole report.** The script prints one line per step and
closes with a summary line; a green full slate is about fifteen lines. Do not
re-run it under `tail`, `grep`, or `head` to find something — a passing step has
already reduced its output to the count, score, or delta worth keeping, and a
failing step has already replayed its output in full. `KOAN_VERBOSE=1` replays
every step's output when a passing step's own numbers genuinely aren't enough.

## Two slates, picked by change scope

The script inspects every path differing from `HEAD` (staged, unstaged, and untracked) and picks one of two slates. Nothing to configure — just report which one ran.

- **Full slate** (9 steps) whenever any changed path sits outside `workgraph/`, and on a clean tree. koan compiling is a gate, as are coverage and the modgraph score.
- **Library slate** (3 steps) when *every* changed path is under `workgraph/`: `cargo test -p workgraph` (unit tests and doctests, `--features test-hooks`), clippy on the same, and doclinks. It then reports whether koan still compiles **as information, never as a gate.**

The library slate exists so a workgraph change can land ahead of koan's adoption of it — see [the library roadmap's convention](../../../workgraph/roadmap/README.md). koan failing to compile against workgraph `HEAD` is the expected mid-migration state there, so treat the reported error count as the size of the debt now owed, not as a failure to fix before committing. Coverage, snippets, the allocation audit, and the modgraph score are koan-rooted and do not run; the trend logs are not rebaselined.

## Reporting the result

The script's final line is the report. **Quote it to the user verbatim** rather
than reassembling one from the step lines. Full slate:

```
Verify: tests ok, doctests ok, clippy clean, doclinks ok, snippets 89/89, allocation audit 7 bounds ok, coverage 91.35% (Δ -0.00 vs 91.35%), modgraph tests ok, modgraph score 6749.00 (Δ +0.00 vs 6749.00).
```

Library slate — it names its own scope, so it is never mistaken for a full run:

```
Verify (workgraph only): tests ok, clippy clean, doclinks ok, koan compiles.
```

Two things the line does not carry, which are worth adding in your own words
when they appear:

- **Sub-lines under a step.** The allocation audit prints the shape and term
  rows that moved and the bounds that drifted; modgraph prints its coupling /
  nesting / size split. A step with nothing to add prints no sub-lines at all,
  so any that appear are the readings that changed.
- **A modified working tree.** `clippy clean after --fix` means clippy applied
  fixes; the tree now differs from what you handed it.

On a failure the slate stops there, so the summary line ends on the failed step
(`… doclinks FAILED.`) and the steps after it never ran — say so rather than
implying they passed. The failing step's full output sits directly above the
summary line; report the substance of it, not just the clause.

## What this skill does *not* do

- **Miri.** The audit slate is separately gated and slow; use the `miri` skill when you need memory-safety verification.
- **`cargo fmt`.** Format drift isn't gated here. Run `cargo fmt --all` separately when needed.
- **Rebaseline the trend logs.** Coverage, the allocation record, and the modgraph score report a delta against the newest recorded entry but write nothing; only `KOAN_REBASELINE` (which pre-commit sets) records a new one.
- **Gate on the modgraph score or the allocation audit.** Both report; neither fails the run. Use the deltas as input to a code-review judgment call, and report them to the user.
