---
name: verify-koan
description: Use this skill to run a koan build-verification tier — routine (`tools/verify.sh`) or total (`tools/verify.sh --total`). Invoke before handing off to the shepherd agent or whenever the user says "verify the build", "run checks", or "is this green?". The routine tier does *not* run Miri, coverage or the modgraph score; the total tier does.
---

# verify-koan

```sh
tools/verify.sh
```

The routine tier runs in the pre-commit hook, so there is no need to run it immediately before
committing. `tools/verify.sh --total` is the deep slate: coverage, Miri, the modgraph score and a
32× property sweep, in minutes rather than seconds.

Read [`tools/verify.sh`](../../../tools/verify.sh) for what runs and in what order.

**One invocation is the whole report.** The script prints one line per step and
closes with a summary line; a green full slate is about fifteen lines. Do not
re-run it under `tail`, `grep`, or `head` to find something — a passing step has
already reduced its output to the count, score, or delta worth keeping, and a
failing step has already replayed its output in full.

**Every step's whole output is in `scratch/verify.log`**, passing or not, which
each run starts afresh and names under the summary line. When you need more
than the summary — a failure's detail you trimmed away, a passing step's own
numbers — read the log; never run the slate again to see it. This holds for the
pre-commit hook too, however its output was piped.

## Two tiers, and three scopes within the routine one

The tier is the argument; the scope is read from the changed paths. Nothing to configure — just
report which ran.

- **Routine** (`tools/verify.sh`, the default and what the pre-commit hook runs): slate-audit,
  tests (unit, integration and doctests in one pass), a warning-free `--release` check of the
  workspace, the tutorial snippets, the cellgraph surface pair, clippy, doclinks.
  Property laws run at 64 cases. It measures nothing — no coverage, no modgraph, no Miri — and
  costs seconds.
- **Total** (`tools/verify.sh --total`, what CI runs): the routine steps plus coverage, Miri over
  the audit slate, the modgraph tooling tests and the complexity score, with property laws at 2048
  cases. Minutes. Run it before a merge, or when the user asks for the deep slate.

Within the routine tier the script inspects every path differing from `HEAD` (staged, unstaged and
untracked) and narrows itself:

- **Markdown only** — doclinks alone. A change that cannot reach a build gets the link audit and
  nothing else.
- **`cellgraph/` only** — that crate's tests, clippy and doclinks, then it reports whether koan
  still compiles **as information, never as a gate**. This is what lets a library change land
  ahead of its adoption: treat a reported error count as the size of the adoption debt now owed,
  not as a failure to fix before committing.
- **Anything else** — the whole workspace, where every crate compiling is a gate.

## Reporting the result

The script's final line is the report. **Quote it to the user verbatim** rather
than reassembling one from the step lines. It names its own tier and scope, so a narrowed run is
never mistaken for a full one:

```
Verify (routine): slate ok, tests ok, release ok, snippets ok, surface ok under --release, surface profile-free, clippy clean, doclinks ok.
Verify (docs only): doclinks ok.
Verify (cellgraph only): tests ok, surface ok under --release, surface profile-free, clippy clean, doclinks ok, koan compiles.
Verify (total): slate ok, tests ok, doctests ok, release ok, surface ok under --release, surface profile-free, clippy clean, doclinks ok, miri ok, coverage 87.18% (Δ -0.00 vs 87.18%), modgraph tests ok, modgraph score 1809.79 (Δ +0.00 vs 1809.79).
```

Two things the line does not carry, which are worth adding in your own words
when they appear:

- **Sub-lines under a step.** modgraph prints its coupling / nesting / size
  split. A step with nothing to add prints no sub-lines at all, so any that
  appear are the readings that changed.
- **A modified working tree.** `clippy clean after --fix` means clippy applied
  fixes; the tree now differs from what you handed it.

On a failure the slate stops there, so the summary line ends on the failed step
(`… doclinks FAILED.`) and the steps after it never ran — say so rather than
implying they passed. The failing step's full output sits directly above the
summary line; report the substance of it, not just the clause.

## What this skill does *not* do

- **Miri, coverage or the modgraph score — in the routine tier.** All three are total-tier steps. Use `tools/verify.sh --total` when the user wants them, or the `miri` skill for a targeted memory-safety question.
- **`cargo fmt`.** Format drift isn't gated here. Run `cargo fmt --all` separately when needed.
- **Rebaseline the trend logs.** The total tier's coverage and modgraph steps report a delta against the newest recorded entry but write nothing; only `KOAN_REBASELINE=1` records a new one, and the routine tier has no reading to record.
- **Gate on the modgraph score.** It reports; it never fails the run. Use the delta as input to a code-review judgment call, and report it to the user.
