---
name: documentation
description: Use this skill for any work touching README.md, TUTORIAL.md, ROADMAP.md, design/*.md, or roadmap/*.md — including editing, adding, deleting, renaming, or auditing docs. Reach for it before deleting or renaming a doc, after editing one, when adding a roadmap item, when finishing a PR (to delete the now-shipped roadmap item), or when the user asks to "audit", "verify", or "fix" doc links / cross-references / roadmap dependencies. Pairs with the `tools/doclinks.py` CLI for cross-reference validation.
---

# documentation

Rules and tools for the koan repo's documentation tree. Three things live here:

1. **The doc partition** — what goes where, and what *not* to put in each location.
2. **Workflow rules** — the discrete actions doc work breaks into, with the gating tool calls each needs.
3. **The doclinks CLI** — `tools/doclinks.py`, which makes the rules verifiable.

## Doc partition

Each tier has a job. Keeping them disjoint is what lets a reader pick the right one without grep.

- **`README.md`**. Introduces a new user or developer to the project. Links other docs. Explains the directory structure. Aim for "what is this and where do I go next."
- **`TUTORIAL.md`**. Walks a user through writing koan code. From the user's perspective, not the implementer's.
- **`TEST.md`**. Testing and linting reference: `cargo test` conventions, clippy/fmt commands, and the canonical Miri audit-slate test list. Updated whenever a slate test is added or removed.
- **`design/*.md`**. Describes shipped behavior — architecture, cross-cutting concerns, design rationale for what's already in the language. Update after a PR is code-complete and tested, but only if a design decision was made. If a file is being deleted or downsized, update inbound references.
- **`roadmap/*.md`**. **Future work only.** Each file is one work item with a `## Dependencies` section. Items can reference prerequisites and unblockers.
- **`ROADMAP.md`**. A curated index of `roadmap/*.md` items. The "What's shipped so far" prose paragraph is the running record of completed work; the bulleted "Open items" sections list active items.
- **Source-file doc comments**. Top-of-file comments explain a file's purpose, assumptions, and links to design docs. Inline comments stay 3–4 lines max; longer rationales belong in design docs. These are kept up-to-date during implementation, not at the doc-update phase — see Claude.md for the implementation-time rules.

## Section structure

Within a roadmap or design file, each named section has a specific job. Mixing the jobs is the most common drift mode — a "Problem" that lists payoffs, an "Impact" that re-states the problem in different words. Keep them disjoint.

### Roadmap files (`roadmap/*.md`)

Standard skeleton (sections may be omitted when they have nothing to say, but their meanings are fixed):

- **`# Title`** — Names the work item. Short noun phrase, not a sentence.
- **Optional one-line lede** — A single sentence framing the item. Skip if the title is self-evident.
- **`**Problem.**`** — The status quo and what's broken or missing about it. Present-tense description of *today's* state, with concrete pointers to the code or doc surface that exhibits the gap. This is the only section that talks about today's pain. If this section starts feeling like a list of payoffs, those belong under Impact.
- **`**Impact.**`** — What shipping this work *buys* the language when done. Frame each bullet as a capability that becomes available, not a limitation that exists today. Read every bullet as completing the sentence "After this ships, …". Cross-stage substrate ("substrate for stage N") and downstream-unblocking effects belong here too. If a bullet starts with "No …" or "… can't …", it's drifted into Problem framing — flip it.
- **`**Directions.**`** — Design choices for how to ship the work. Each bullet is a *decision point*; Problem/Impact cover the *what* and *why*.

  **Every bullet carries an explicit status tag** in its first phrase: `— decided.` (with optional `per <link>`), `— open.` (lists alternatives; may add `Recommended: <option>.`), or `— deferred.` (punted to a named follow-up). An untagged bullet is the drift the audit catches. Bullets that aren't decision points (constraints, obligations) belong in Impact or Problem.

  The `**Directions.**` heading takes no lede — no "None decided", no "X decided per Y; rest open" preamble. The per-bullet tags are the canonical signal; a lede only duplicates them and goes stale.
- **`## Dependencies`** — Two labelled bullet lists:
  - **`Requires:`** — Prerequisites: roadmap items that must ship first, or shipped substrate the work depends on. Each bullet is a markdown link plus an optional inline rationale.
  - **`Unblocks:`** — Downstream items this work makes possible. Symmetric with Requires across files (`A.md` listing `B.md` under Requires obliges `B.md` to list `A.md` under Unblocks). The doclinks `deps` subcommand verifies the symmetry.

  Either list may be absent or "none" when the item is a foundation or a leaf. A trailing prose paragraph can capture nuance the bullets can't carry (soft prerequisites, ordering preferences).

### Design files (`design/*.md`)

Design files describe **the design** — shipped or aspirational — in whatever section shape the topic wants. There is no fixed section order, but the framing rules are strict:

- **Body prose describes the design as it is or as it will be.** Architecture, invariants, decisions and their rationale, code pointers. Aspirational design is fine: a design doc can land before its implementation, capturing the shape the work will take. The doc reads as a single coherent picture of "this is how it works (or will work)" — present-tense or future-tense, never both side-by-side.
- **No historical narrative.** Design docs never reference past designs, prior approaches, or earlier states the language passed through. "Used to be X, now is Y" comparisons belong in `git log` and PR descriptions, not in the doc tree. If a body section starts re-litigating a superseded design, delete the comparison and keep only the current shape.
- **Future work points to roadmap, always.** A `## Open work` (or `## Open issues`, `## Outstanding`) section at the bottom is the only place a design doc looks forward beyond its own design statement, and every entry there is a link to a `roadmap/*.md` item. No inline TODOs, no "we should eventually" prose elsewhere — open-ended future work is roadmap territory by partition.

The partition rule: **design docs explain the design; roadmap docs explain the work to do.** When a roadmap item ships, its narrative content (if any) migrates into a design doc body section as part of the existing design — not as an "Open work" entry, and not as a "previously this didn't exist" historical note.

## Workflow rules

These are the rules that don't survive on vibes; they need explicit tool gates.

### When a roadmap item ships

The work item is no longer future work, so it leaves `roadmap/`.

1. **Run `python3 tools/doclinks.py rm-roadmap roadmap/<item>.md`** (use `--dry-run` first if unsure). The tool deletes the file, prunes intra-roadmap dependency bullets, and strips the entry from `ROADMAP.md`'s `## Next items` and `## Open items` subsections. It does **not** touch design-doc prose, source comments, or the "What's shipped so far" paragraph — those are judgment calls.
2. **Run `python3 tools/doclinks.py refs roadmap/<item>.md`** before the delete (or run `check` after) to surface the references the tool can't auto-handle: design-doc "Open work" entries, source-file `//` comments, prose mentions inside Dependencies sections.
3. Update each remaining callsite. A `design/` doc whose "Open work" pointed to the item gets either a body section describing what shipped, or the open-work bullet removed.
4. If the shipped behavior has explanatory value not already captured, **move that narrative into the relevant `design/*.md`** as a body section (not as an "Open work" entry — that section is for what's still future).
5. **Update `ROADMAP.md` prose:** add a phrase to the "What's shipped so far" paragraph naming what landed. (The tool only touches the bullet lists.)
6. **Re-run `doclinks check`** (below).

### After a Miri run in this session

If the session ran a Miri full-slate invocation, update the slate-duration log
in [`TEST.md`](../../../TEST.md) before any other doc work:

1. Locate the `<!-- slate-durations:start -->` / `<!-- slate-durations:end -->`
   block.
2. Prepend a new bullet for the most recent full-slate run, format
   `- YYYY-MM-DD: <duration>s — <N> tests, <leaks> leaks, <ub> UB`.
3. Trim the list to the five most-recent entries.

The miri skill carries the parsing rules for `<duration>` / `<N>` / `<leaks>`
/ `<ub>` from Miri's output; this rule is just the doc-side update obligation.
Single-test triage runs and non-slate Miri invocations do not trigger this.

### When editing any doc

After the edit:
- `python3 tools/doclinks.py check` — runs the four audits in one pass: broken links, roadmap `Requires`/`Unblocks` symmetry, orphaned docs, and the informational source-tree-changes report.
- **Audit the edited file against the Section structure rules above.** For a `roadmap/*.md` file, confirm Problem describes today's state (not payoffs), Impact reads as "what shipping this buys" (no "No …" / "… can't …" bullets), every Directions bullet carries a `— decided.` / `— open.` / `— deferred.` tag, and `## Dependencies` has both edges where they apply. For a `design/*.md` file, confirm the body has no historical narrative ("was fixed," "old behavior," "previously," "earlier designs called …") and no forward-looking prose outside `## Open work` (`## Open work` is the only place future work appears, and every entry links to a `roadmap/*.md` item). Fix any drift in the same edit; partition violations compound silently.

### Before deleting or renaming any doc

Even non-roadmap docs:
1. `python3 tools/doclinks.py refs <path>` to find every file that links to it.
2. Update each callsite. For bulk renames, `python3 tools/doclinks.py fix-refs OLD=NEW [...]` rewrites every link whose target resolves to OLD across markdown and rust comments in one pass; pass `--dry-run` to preview, or `--from-file` for a list.
3. Move/delete the file.
4. `python3 tools/doclinks.py check` to confirm zero new broken links.

### When adding a new roadmap item

1. Create `roadmap/<new-item>.md` with a `## Dependencies` section listing `Requires:` and/or `Unblocks:` edges.
2. Add a bullet to `ROADMAP.md` under the right "Open items" subsection.
3. If the new item is unblocked by something, add a back-edge `Unblocks: <new-item>` in that prerequisite item's Dependencies.
4. Run `python3 tools/doclinks.py check` to confirm bidirectional dependency symmetry and that the new file is wired in (the orphans section will flag an unreferenced doc).

### PR-end audit

A single command runs every audit:

```sh
python3 tools/doclinks.py check
```

Run this even on PRs that "didn't touch docs" — a renamed source file can break a top-of-file link from a design doc.

The command prints four sections under `##` headers: broken links, roadmap dependency symmetry, orphaned docs, and source-tree changes vs `master`. The first three are gates — any failure flips the exit code. The source-tree section is informational: it lists every `src/**/*.rs` file added, modified, deleted, or renamed since `master`, annotated with every inbound doc link, so the caller can decide which changed files warrant a `README.md` "Source layout" update or a design-doc edit. Pass `--base <ref>` to diff against something other than `master`.

## doclinks CLI

A Python CLI at `tools/doclinks.py`. Run with `python3 tools/doclinks.py <subcommand>` from the repo root.

### `check` — run all four audits

Prints four sections under `##` headers and returns a single exit code. The first three are gates; the fourth is informational.

- **broken links.** Scans every `*.md` file plus `//` comments in `src/**/*.rs` for `[text](path)` links and reports any whose target doesn't exist on disk. URL fragments (`#anchor`) and rustdoc intra-doc links (`super::foo`, `crate::a::b`) are filtered out.
- **roadmap dependencies.** Parses the `## Dependencies` section of every `roadmap/*.md` file and confirms every edge is bidirectional: `A.md` listing `B.md` under **Requires:** obliges `B.md` to list `A.md` under **Unblocks:**, and vice versa. Catches the easy mistake of updating one side of a dependency edge.
- **orphaned design/ + roadmap/ docs.** Every `design/*.md` and `roadmap/*.md` file that no other doc, comment, or source file links to. An orphan is usually either a new doc that needs an entry in `README.md` / `ROADMAP.md`, or a stale doc that should be deleted.
- **source-tree changes vs the base ref** (informational, never gates). Every `src/**/*.rs` file added, modified, deleted, or renamed since the base, annotated with each inbound doc link. The working tree is compared to the base, so committed and uncommitted edits surface in one pass. Renames show inbound links to *both* the old path (broken until rewritten) and the new path. Use this to spot source files that may need a `README.md` "Source layout" entry, a design-doc reference, or a `fix-refs` pass to fix renamed-path links — the caller decides which warrant action.

The exit code is non-zero if any of the three gating sections flags an issue. Pass `--base <ref>` to change what the source-tree section diffs against (default: `master`).

```sh
python3 tools/doclinks.py check
python3 tools/doclinks.py check --base HEAD~5
```

### `refs <path>` — list everything that links to a file

Before renaming or deleting a doc, run this to see who references it. Prints `file:line: [text](target)` for every match.

```sh
python3 tools/doclinks.py refs design/execution-model.md
python3 tools/doclinks.py refs roadmap/traits.md
```

### `fix-refs OLD=NEW [...]` — fix inbound references after a file move

Bulk-fix broken links after a file move or rename. Each `OLD=NEW` is a pair of repo-relative paths; the tool finds every link whose target resolves to `OLD` (across markdown and rust `//` comments) and rewrites it to point at `NEW`, preserving any `#fragment` or `?query` suffix. By default, **the visible `[text]` is also rewritten** to match `NEW`'s `# Heading` H1 title whenever the two differ — this fixes the common case of a renamed doc whose callsites still carry the old title. Pass `--keep-text` to preserve every link's existing text instead. Refuses to run if any `NEW` doesn't exist on disk. Use `--dry-run` to preview; the dry-run output annotates each text rewrite as `(text: 'old' -> 'new')` so in-prose links (e.g., "see [our spec](path)") that should *not* take the H1 are easy to spot before commit. Pass `--from-file mapping.txt` for a long list (one `OLD=NEW` per line, blank lines and `#` comments allowed). Mappings apply against the original resolved target only — chained mappings (`A=B`, `B=C`) need to be expressed as the final target.

```sh
python3 tools/doclinks.py fix-refs --dry-run \
  src/dispatch/scope.rs=src/dispatch/runtime/scope.rs
python3 tools/doclinks.py fix-refs --from-file dispatch-refactor.txt
```

### `rm-roadmap <roadmap/item.md>` — delete a roadmap item with cleanup

When a roadmap item ships, this command deletes the file *and* prunes its inbound bullets in a single pass: every `**Requires:**` / `**Unblocks:**` bullet in other `roadmap/*.md` items that links to it, plus any bullet under `## Next items` or `## Open items` in `ROADMAP.md`. Continuation lines belonging to a removed bullet are dropped too. After the delete it runs `check` automatically and propagates its exit code, so any remaining stale references (design-doc "Open work" sections, source-file comments, prose mentions inside Dependencies sections) surface immediately. The tool does **not** touch the `What's shipped so far` paragraph or those remaining-reference sites — those need a judgment call. Use `--dry-run` to preview.

```sh
python3 tools/doclinks.py rm-roadmap --dry-run roadmap/transient-node-reclamation.md
python3 tools/doclinks.py rm-roadmap roadmap/transient-node-reclamation.md
```

## Anti-patterns

- **Don't `grep` for cross-references when `doclinks refs` would do it correctly.** `grep` doesn't resolve relative paths, doesn't catch asymmetric `Requires`/`Unblocks` edges, doesn't catch rustdoc-style links in source comments, and doesn't surface orphans.
- **Don't keep a roadmap entry "as a record" of shipped work** — including marking it "— shipped" or rewriting it as a resolution summary. That's what `git log` plus `design/` is for.

## Notes

- Paths in markdown links are resolved relative to the file the link appears in, so the tool handles both `[…](design/x.md)` from the repo root and `[…](x.md)` from within `design/`.
- The CLI walks the working tree from `tools/doclinks.py`'s parent, so it works regardless of cwd as long as the script stays at `tools/doclinks.py`.
- For Rust source files, only lines containing `//` are scanned — `[x](y)` inside string literals is intentionally ignored.
