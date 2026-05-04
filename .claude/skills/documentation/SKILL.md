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
- **`design/*.md`**. Describes shipped behavior — architecture, cross-cutting concerns, design rationale for what's already in the language. Update after a PR is code-complete and tested, but only if a design decision was made. If a file is being deleted or downsized, update inbound references.
- **`roadmap/*.md`**. **Future work only.** Each file is one work item with a `## Dependencies` section. Items can reference prerequisites and unblockers.
- **`ROADMAP.md`**. A curated index of `roadmap/*.md` items. The "What's shipped so far" prose paragraph is the running record of completed work; the bulleted "Open items" sections list active items.
- **Source-file doc comments**. Top-of-file comments explain a file's purpose, assumptions, and links to design docs. Inline comments stay 3–4 lines max; longer rationales belong in design docs. These are kept up-to-date during implementation, not at the doc-update phase — see Claude.md for the implementation-time rules.

## Workflow rules

These are the rules that don't survive on vibes; they need explicit tool gates.

### When a roadmap item ships

The work item is no longer future work, so it leaves `roadmap/`.

1. **Run `python3 tools/doclinks.py rm-roadmap roadmap/<item>.md`** (use `--dry-run` first if unsure). The tool deletes the file, prunes intra-roadmap dependency bullets, and strips the entry from `ROADMAP.md`'s `## Next items` and `## Open items` subsections. It does **not** touch design-doc prose, source comments, or the "What's shipped so far" paragraph — those are judgment calls.
2. **Run `python3 tools/doclinks.py refs roadmap/<item>.md`** before the delete (or run `check` after) to surface the references the tool can't auto-handle: design-doc "Open work" entries, source-file `//` comments, prose mentions inside Dependencies sections.
3. Update each remaining callsite. A `design/` doc whose "Open work" pointed to the item gets either a body section describing what shipped, or the open-work bullet removed.
4. If the shipped behavior has explanatory value not already captured, **move that narrative into the relevant `design/*.md`** as a body section (not as an "Open work" entry — that section is for what's still future).
5. **Update `ROADMAP.md` prose:** add a phrase to the "What's shipped so far" paragraph naming what landed. (The tool only touches the bullet lists.)
6. **Re-run the gating triple** (below).

### When editing any doc

After the edit:
- `python3 tools/doclinks.py check` — find broken links.
- `python3 tools/doclinks.py deps` — verify roadmap `Requires`/`Unblocks` symmetry if you touched a `## Dependencies` section.

### Before deleting or renaming any doc

Even non-roadmap docs:
1. `python3 tools/doclinks.py refs <path>` to find every file that links to it.
2. Update each callsite. For bulk renames, `python3 tools/doclinks.py rewrite OLD=NEW [...]` rewrites every link whose target resolves to OLD across markdown and rust comments in one pass; pass `--dry-run` to preview, or `--from-file` for a list.
3. Move/delete the file.
4. `python3 tools/doclinks.py check` to confirm zero new broken links.

### When adding a new roadmap item

1. Create `roadmap/<new-item>.md` with a `## Dependencies` section listing `Requires:` and/or `Unblocks:` edges.
2. Add a bullet to `ROADMAP.md` under the right "Open items" subsection.
3. If the new item is unblocked by something, add a back-edge `Unblocks: <new-item>` in that prerequisite item's Dependencies.
4. Run `python3 tools/doclinks.py deps` to confirm bidirectional symmetry.
5. Run `python3 tools/doclinks.py orphans` to confirm the new file is wired in.

### PR-end audit

The three exit-coded gates, all must pass:

```sh
python3 tools/doclinks.py check && python3 tools/doclinks.py deps && python3 tools/doclinks.py orphans
```

Run this even on PRs that "didn't touch docs" — a renamed source file can break a top-of-file link from a design doc.

## doclinks CLI

A Python CLI at `tools/doclinks.py`. Run with `python3 tools/doclinks.py <subcommand>` from the repo root.

### `check` — find broken links

Scans every `*.md` file plus comments in `src/**/*.rs` for `[text](path)` links and reports any whose target doesn't exist on disk. URL fragments (`#anchor`) and rustdoc intra-doc links (`super::foo`, `crate::a::b`) are filtered out. Exits non-zero if any link is broken.

### `deps` — verify roadmap dependency symmetry

Parses the `## Dependencies` section of every `roadmap/*.md` file and confirms every edge is bidirectional: if `A.md` lists `B.md` under **Requires:**, then `B.md` must list `A.md` under **Unblocks:** (and vice versa). Catches the easy mistake of updating one side of a dependency edge and forgetting the other. Exits non-zero on any asymmetry.

### `orphans` — find unreferenced docs

Lists every `design/*.md` and `roadmap/*.md` file that no other doc, comment, or source file links to. An orphan is usually either a new doc that needs an entry in `README.md` / `ROADMAP.md`, or a stale doc that should be deleted.

### `refs <path>` — list everything that links to a file

Before renaming or deleting a doc, run this to see who references it. Prints `file:line: [text](target)` for every match.

```sh
python3 tools/doclinks.py refs design/execution-model.md
python3 tools/doclinks.py refs roadmap/traits.md
```

### `rewrite OLD=NEW [...]` — apply path-mapping rewrites

Bulk-fix broken links after a file move or rename. Each `OLD=NEW` is a pair of repo-relative paths; the tool finds every link whose target resolves to `OLD` (across markdown and rust `//` comments) and rewrites it to point at `NEW`, preserving any `#fragment` or `?query` suffix and the visible `[text]`. Refuses to run if any `NEW` doesn't exist on disk. Use `--dry-run` to preview; pass `--from-file mapping.txt` for a long list (one `OLD=NEW` per line, blank lines and `#` comments allowed). Mappings apply against the original resolved target only — chained mappings (`A=B`, `B=C`) need to be expressed as the final target.

```sh
python3 tools/doclinks.py rewrite --dry-run \
  src/dispatch/scope.rs=src/dispatch/runtime/scope.rs
python3 tools/doclinks.py rewrite --from-file dispatch-refactor.txt
```

### `rm-roadmap <roadmap/item.md>` — delete a roadmap item with cleanup

When a roadmap item ships, this command deletes the file *and* prunes its inbound bullets in a single pass: every `**Requires:**` / `**Unblocks:**` bullet in other `roadmap/*.md` items that links to it, plus any bullet under `## Next items` or `## Open items` in `ROADMAP.md`. Continuation lines belonging to a removed bullet are dropped too. After the delete it runs `check` automatically and propagates its exit code, so any remaining stale references (design-doc "Open work" sections, source-file comments, prose mentions inside Dependencies sections) surface immediately. The tool does **not** touch the `What's shipped so far` paragraph or those remaining-reference sites — those need a judgment call. Use `--dry-run` to preview.

```sh
python3 tools/doclinks.py rm-roadmap --dry-run roadmap/transient-node-reclamation.md
python3 tools/doclinks.py rm-roadmap roadmap/transient-node-reclamation.md
```

## Anti-patterns

- **Don't `grep` for cross-references when `doclinks refs` would do it correctly.** `grep` doesn't resolve relative paths (`../` from a `design/` file vs. a repo-root path), doesn't catch asymmetric `Requires`/`Unblocks` edges, doesn't catch rustdoc-style links in source comments, and doesn't surface orphans. Every minute saved by reaching for `grep` is paid back fivefold by the silent breakage that survives.
- **Don't keep a roadmap entry "as a record" of shipped work.** That's what `git log` plus `design/` is for. The roadmap is for future work.
- **Don't add forward-compatibility shims, migration notes, or backward-compatibility hedges.** Koan is pre-release with no users; deprecation paths and version-N-to-N+1 notes are pure overhead.
- **Don't sacrifice grammar for brevity.** Concise prose still uses complete sentences.
- **Don't mark a roadmap item "— shipped" instead of deleting it.** Same partition violation as keeping it as a record.

## Notes

- Paths in markdown links are resolved relative to the file the link appears in, so the tool handles both `[…](design/x.md)` from the repo root and `[…](x.md)` from within `design/`.
- The CLI walks the working tree from `tools/doclinks.py`'s parent, so it works regardless of cwd as long as the script stays at `tools/doclinks.py`.
- For Rust source files, only lines containing `//` are scanned — `[x](y)` inside string literals is intentionally ignored.
