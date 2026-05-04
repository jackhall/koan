---
name: doclinks
description: Use this skill to maintain links between documentation files in the koan repo (README.md, ROADMAP.md, design/, roadmap/, and source-file doc comments). Reach for it after editing any doc, before deleting or renaming a doc, when adding a roadmap item, or when the user asks to "audit", "verify", or "fix" doc links / cross-references / roadmap dependencies.
---

# doclinks

A Python CLI at `tools/doclinks.py` for keeping the repo's documentation cross-references consistent. Run it with `python3 tools/doclinks.py <subcommand>` from the repo root.

## Subcommands

### `check` — find broken links
Scans every `*.md` file plus comments in `src/**/*.rs` for `[text](path)` links and reports any whose target doesn't exist on disk. URL fragments (`#anchor`) and rustdoc intra-doc links (`super::foo`, `crate::a::b`) are filtered out. Exits non-zero if any link is broken.

```sh
python3 tools/doclinks.py check
```

### `deps` — verify roadmap dependency symmetry
Parses the `## Dependencies` section of every `roadmap/*.md` file and confirms every edge is bidirectional: if `A.md` lists `B.md` under **Requires:**, then `B.md` must list `A.md` under **Unblocks:** (and vice versa). Catches the easy mistake of updating one side of a dependency edge and forgetting the other. Exits non-zero on any asymmetry.

```sh
python3 tools/doclinks.py deps
```

### `orphans` — find unreferenced docs
Lists every `design/*.md` and `roadmap/*.md` file that no other doc, comment, or source file links to. An orphan is usually either a new doc that needs an entry in `README.md` / `ROADMAP.md`, or a stale doc that should be deleted.

```sh
python3 tools/doclinks.py orphans
```

### `refs <path>` — list everything that links to a file
Before renaming or deleting a doc, run this to see who references it. Prints `file:line: [text](target)` for every match.

```sh
python3 tools/doclinks.py refs design/execution-model.md
python3 tools/doclinks.py refs roadmap/traits.md
```

## When to use

- **After editing a design doc or roadmap item:** run `check` and `deps`.
- **Before deleting or renaming a doc:** run `refs <path>` first, then update each callsite, then run `check` to confirm.
- **After adding a new roadmap item:** run `deps` (catches missed Requires/Unblocks back-edges) and `orphans` (confirms the new file is wired into `ROADMAP.md` or some other doc).
- **As a one-shot doc audit:** run `check && deps && orphans` — three exit-coded gates suitable for a quick pre-commit pass.

## Notes

- Paths in links are resolved relative to the file the link appears in (markdown convention), so the tool handles both `[…](design/x.md)` from the repo root and `[…](x.md)` from within `design/`.
- The CLI walks the working tree from `tools/doclinks.py`'s parent, so it works regardless of cwd as long as the script stays at `tools/doclinks.py`.
- For Rust source files, only lines containing `//` are scanned. This means it ignores `[x](y)` inside string literals, which is intentional — those aren't real cross-references.
