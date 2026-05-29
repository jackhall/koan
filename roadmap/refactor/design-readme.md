# `design/README.md` — design-tree index + foundation/seam heuristic

Write `design/README.md` as the design tree's entry point: a per-file
index naming what each design doc primarily owns, the
foundation-vs-seam heuristic that distinguishes "correctly cited
foundation" from "missing seam," and pointers to the analysis
tooling (`modgraph`, `doclinks gap`, `modgraph_rewrite item`) for
future refactor passes.

**Problem.** No single artifact explains how the `design/` tree is
organized. A new contributor finds 20+ markdown files with no
authoritative answer to "which doc owns what." The
`doclinks signals` audit's `unowned_concepts` section surfaces this
at the file level (`scope.rs` mentioned in 11 docs, top doc holds
only 24% of mentions), but the tree-level partition — what each doc
is *primarily about* — has no documented owner. Future refactor
passes (the kind that produced #16–#18) discover ad-hoc which doc to
update because the partition isn't stated anywhere.

The candidates analysis also surfaced a methodology that worked
across 17 passes and four tooling fixes: distinguish a *foundation*
(correctly cited everywhere because every operation goes through it
— `scope.rs`, `bindings.rs`, `arena.rs`) from a *seam* (concept
restated across docs because no source file owns the contract —
nominal dual-write, per-call arena protocol). The `modgraph` metric
plus `doclinks gap` can tell these apart, but the heuristic itself
lives only in the analysis scratch file; the next contributor doing
refactor analysis builds it from scratch.

**Impact.**

- `design/README.md` answers "what does each doc own?" with a
  per-file bullet. Future doc edits land in the right doc by
  partition instead of by intuition; the `unowned_concepts` signal
  becomes resolvable by reading the index.
- The foundation/seam heuristic is captured in one place. Future
  refactor analysis starts from "is this a foundation or a seam?"
  rather than reconstructing the framing from scratch — avoiding
  the `core::lookup` / `core::scope` trap of treating a
  correctly-distributed foundation as a hidden seam.
- The analysis tooling chain (`modgraph` + `--reference-loc` +
  `doclinks gap` + `modgraph_rewrite item --delete` /
  `--delete-file`) has a documented entry point. New refactor
  candidates can be scored against the current baseline without
  re-deriving the methodology.
- The design tree gets a real entry point for the `README.md`'s
  `## Source layout` cross-link, completing the partition
  README ⇄ TUTORIAL ⇄ design index ⇄ roadmap index.

**Directions.**

- **Three sections in `design/README.md` — decided.**
  1. **Doc index.** One bullet per file in `design/` and
     `design/typing/`, naming what concept it primarily owns.
     Resolves the `unowned_concepts` signal at the tree level.
  2. **Foundation vs seam heuristic.** Short note distinguishing
     "foundation = correctly cited everywhere because every
     operation goes through it" from "seam = concept restated
     across docs because no source file owns the contract." Cite
     the candidates analysis's worked examples (`scope.rs` as
     foundation, nominal dual-write as seam).
  3. **Analysis tooling pointers.** Brief mention of `modgraph` +
     `doclinks gap` + `modgraph_rewrite item` with the canonical
     command for "score a candidate against the current baseline."
- **Index format — decided.** Markdown bulleted list per directory,
  one line per file, format `<link-to-doc> — one-line concept-owner
  statement`. Same shape as `ROADMAP.md`'s "Open items" subsections
  so the two indexes feel like siblings.
- **Heuristic length — decided.** One paragraph each for
  foundation and seam, two paragraphs total. The candidates
  analysis is the long-form expansion; this captures the operational
  test.

## Dependencies

**Requires:** none.

**Unblocks:** none.
