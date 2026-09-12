# Design docs in the modules they describe

Each kept module carries its own design doc as a `README.md` in its directory,
and the retired `old_design/` tree holds only docs for code the rewrite replaces.

**Problem.** Design docs live in one flat tree apart from the code:
[old_design/](../../old_design/README.md) for koan and
[cellgraph/design/](../../cellgraph/design/) for the cell substrate. The docs
that describe kept modules sit there beside docs for the runtime the rewrite
replaces — `parse` is described by
[expressions-and-parsing.md](../../old_design/expressions-and-parsing.md) and
[label-interning.md](../../old_design/label-interning.md), `type_lattice` by
[typing/type-lattice.md](../../old_design/typing/type-lattice.md) and its
neighbours, `memory` by [memory-model.md](../../old_design/memory-model.md),
[value-substrates.md](../../old_design/value-substrates.md) and
[per-call-region/](../../old_design/per-call-region/README.md) — and the whole
tree carries the `old_` prefix because nothing separates the two. A reader of
`src/parse` finds its design by walking to a different root and searching a
topical index, and a doc that spans several modules has no module to be wrong
about. `doclinks` link-checks a `src/**/README.md` but does not orphan-gate one
or list it anywhere.

**Acceptance criteria.**

- Each kept module — `src/parse`, `src/memory`, `src/type_lattice`, `src/source`
  and `sexlex`, and each of `cellgraph`'s modules that has a design doc — has a
  `README.md` in its directory that is its design doc, and the module's
  top-of-file comment links it.
- `old_design/` and `cellgraph/design/` hold no doc whose subject is a kept
  module: the content moved into the module README, and the tree file is
  deleted.
- A doc that spans modules is split so each module's README carries its own
  part, and the cross-module invariant lives in the README of the module that
  enforces it.
- `doclinks` orphan-gates `src/**/README.md` files across koan and the embedded
  crates, and `README.md`'s "Design and roadmap" section indexes the module
  READMEs rather than a topical tree.
- The documentation skill's partition names the module README as the home of a
  design doc and `old_design/` as retired requirements, and `doclinks check`
  passes.

**Directions.**

- *Granularity of a module README — open.* One README per top-level kept module,
  or one per submodule directory that has a distinct design (`src/parse/forms`,
  `src/type_lattice/walk`). Recommended: start at the top-level module and split
  when a README's sections stop sharing a reader.
- *Boundary docs — decided.* Docs that describe the boundary between koan and a
  replaced crate ([scheduler-library.md](../../old_design/scheduler-library.md),
  [per-node-memory.md](../../old_design/per-node-memory.md),
  [witness-hosting.md](../../old_design/witness-hosting.md)) describe replaced
  code and stay retired; the fresh scheduler's boundary is written new in the
  scheduler item.
- *Cross-cutting language docs — open.* Docs with no module — the tutorial-facing
  rationale in [functional-programming.md](../../old_design/functional-programming.md),
  [effects.md](../../old_design/effects.md) — either stay retired until a
  rewrite module owns them or move to a root-level index. Recommended: stay
  retired; each becomes a module README when the layer that implements it
  lands.

## Dependencies

The values, scheduler and scope items write their design docs under this
convention from the start, so the migration and the new modules never disagree
about where a design doc lives.

**Requires:** none — doc work over shipped modules.

**Unblocks:** none tracked yet.
