---
description: Audit src/**/*.rs for excessive or stale comments and slim them down per Claude.md / documentation-skill rules.
argument-hint: <empty | "all" | path-prefix>
---

You are running a comment-audit sweep across the koan repo's `src/**/*.rs`. Reproduces the workflow that produced commit `ad6dd69` (~1400 lines slimmed across 28 files): survey → scope choice → per-file agent batches → targeted consolidation → verify → commit.

`$ARGUMENTS` is an optional hint:
- empty → ask the scope question normally.
- `all` → skip the scope question and audit every comment-bearing file.
- a path prefix (e.g. `src/parse/`) → restrict the audit to files under that prefix.

**Do not edit source files yourself.** Each file's audit is delegated to a sub-agent so your context stays clean. You only do (a) the survey, (b) post-batch consolidation where source duplicates a design-doc section, (c) verification, and (d) the commit.

## Workflow

### 1. Survey comment density

```bash
find src -name "*.rs" -type f | while read f; do
  total=$(wc -l < "$f")
  comments=$(grep -cE '^\s*//' "$f")
  if [ "$total" -gt 0 ] && [ "$comments" -gt 5 ]; then
    pct=$((comments * 100 / total))
    printf "%3d%% %4dL %3dC %s\n" "$pct" "$total" "$comments" "$f"
  fi
done | sort -rn
```

Also note files carrying lingering markers worth a closer look:

```bash
find src -name "*.rs" | xargs grep -l "TODO\|FIXME\|XXX\|HACK\|NOTE:" 2>/dev/null
```

Show the top of the table to the user.

### 2. Lock scope / migration / commit choices

Use `AskUserQuestion` with three questions before launching anything (skip the scope question if `$ARGUMENTS` already specified it):

1. **Scope** — `~25 files >20% comments` (recommended), `Comment-bearing files only (>5 comment lines)`, or `All .rs files`.
2. **Migration** — `Leave source comment in place; flag only` (recommended) or `Delete source comment immediately`. Recommended preserves rationale until your consolidation pass writes it elsewhere.
3. **Commit strategy** — `Single commit at end after verify` (recommended), per-file commits, or no commit.

### 3. Per-file agent batches

Build the file list from the survey + scope. Launch in batches of ~10 in a single message (multiple `Agent` tool calls in one message run concurrently). Each agent gets the prompt below — fill in `<PATH>` with the absolute path.

For files containing `unsafe` blocks (e.g. `arena.rs`, `scheduler.rs`, `kfunction.rs`, `module.rs`), prepend a one-line note: *"This file contains unsafe transmutes/SAFETY blocks. Do NOT delete SAFETY rationale — it is required for unsafe-code review. Flag verbose blocks as migration candidates instead."*

```
You're auditing one Rust source file in the koan repo (a pre-release programming language interpreter at /var/home/jack/Code/koan) for excessive and out-of-date comments. Single file scope. No design-doc edits.

# Comment rules (from the project's Claude.md and documentation skill)

**Top-of-file comments (`//!` module docs or leading `//`)**: keep them. They explain the file's purpose, key assumptions, and how it relates to other files. Trim if rambly. Verify any `[text](path)` design-doc links resolve to a real file under /var/home/jack/Code/koan/design/ or /var/home/jack/Code/koan/roadmap/. If a link is broken, flag it.

**Inline `//` comments**: cap at 3-4 lines. Default to NO comment unless the WHY is non-obvious (a hidden constraint, a subtle invariant, a workaround for a specific bug, behavior that would surprise a reader).

**`///` doc comments on items**: keep, but apply the same rules — trim WHAT-explanations that the identifier and signature already convey; keep WHY/invariants/safety notes.

**Hard rules:**
- DO NOT explain WHAT the code does — well-named identifiers and signatures do that.
- DO NOT reference the current task, fix, or callers (e.g., "used by X", "added for Y flow", "handles case from issue Z").
- Stale comments (renamed functions, broken design-doc links, obsolete invariants, completed TODOs) MUST be fixed or removed.
- Comments that say "previously..." / "was X, now Y" — delete (project history lives in git, not source).

# Project context
- Pre-release language, NO users — no backward-compat concerns.
- Design docs at design/*.md (shipped behavior). Roadmap at roadmap/*.md (future work).

# Your target
**Audit and edit:** <PATH>

# Procedure
1. Read the file in full.
2. Edit the file in place to remove or rewrite excessive/stale/WHAT-only comments.
3. Trim the top-of-file block to: purpose + key assumptions + design-doc links.
4. **Do not delete a long but load-bearing rationale** (SAFETY block on unsafe code, non-obvious invariant > 4 lines). Leave it in place and flag it as a "migration candidate".
5. **Do not edit any file other than your target.** No design/, no roadmap/, no other source files. Do not run cargo.

# Available design docs (read-only — for verifying links and suggesting migration targets)
design/effects.md, design/error-handling.md, design/execution-model.md, design/expressions-and-parsing.md, design/functional-programming.md, design/memory-model.md, design/module-system.md, design/type-system.md

# Output format
## Result for <PATH>
- **Lines removed:** <int>
- **Stale comments fixed:** <list or "none">
- **Migration candidates:**
  - <PATH>:<line> → suggested target: <design/foo.md or "unsure">
    Verbatim text:
    > <block>
- **Flags:** <broken links, references to types you couldn't find, or "none">
- **Summary:** <one or two sentences>

Be conservative: if a comment looks load-bearing and you're not sure, leave it and flag it.
```

After each batch returns, surface every agent's `## Result for ...` block to the user verbatim (per the feedback memory `feedback_subagent_output_verbatim.md`) — do not paraphrase.

### 4. Aggregate and consolidate

For each migration candidate the agents flagged, decide accept/reject by comparing the verbatim block against the relevant `design/*.md` section:

- **Design doc already covers it (typical case)** — replace the source block with a brief WHY + markdown link to the doc section.
- **Design doc doesn't cover it but should** — invoke the `documentation` skill to refresh the partition rules, then add a section to the design doc and link from source.
- **SAFETY block at an `unsafe` site** — leave inline. Rust review convention is that SAFETY justifications sit at the unsafe operation.

**Path-computation pitfall.** Markdown links from a source file resolve relative to that file's directory. From `src/dispatch/runtime/arena.rs` (depth 3 under `src/`), `design/memory-model.md` is `../../../design/memory-model.md` — count the slashes in the path under `src/`, not including `src/` itself. Off-by-one passes Rust's compiler but fails `doclinks check`.

### 5. Verify

```bash
cargo build --tests
cargo test --no-fail-fast
cargo clippy --tests --quiet
python3 tools/doclinks.py check
```

All four must be clean. `doclinks check` failures are usually broken design-doc paths from step 4 — fix the path, do not paper over with `# fragment` removal. If a pre-existing broken link surfaces (a doc reference unrelated to your edits), fix it opportunistically and call it out separately in the commit message.

### 6. Commit

Per the step-2 commit choice. Single-commit form:

```
slim source comments, link to design docs

Audited <N> source files for excessive and out-of-date comments per
Claude.md / documentation skill rules. Trimmed <total> lines.

Where source duplicated design-doc content, replaced with brief
pointers (<list>). SAFETY blocks at unsafe sites kept inline for
review locality.

Verified: cargo build --tests, cargo test (<X> pass), cargo clippy
clean, python3 tools/doclinks.py check.
```

## When to bail

- Cargo build/test/clippy fails after agents return — surface the failure and stop. Don't repair an agent's mangled edit yourself; re-spawn that file's agent with the failure quoted.
- An agent reports a SAFETY block was deleted instead of flagged — re-run the agent with explicit "do not delete SAFETY blocks" reinforcement.
- `doclinks check` flags a pre-existing broken link — fix it as part of the audit but call it out separately in the commit message.

## Anti-patterns

- **Don't run all files in parallel without batching.** ~10 agents per message keeps tool-result volume manageable.
- **Don't have agents edit design docs directly.** Concurrent edits across N agents to the same `design/*.md` will conflict; consolidation is single-threaded orchestrator work.
- **Don't skip the survey.** A 5%-comment-density file rarely has anything worth slimming; agent runs on those are no-ops that burn tokens.
