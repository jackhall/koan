# Publishing the workgraph crate

**Problem.** The `workgraph` workspace crate is the library boundary — it
compiles with no Koan type in scope, and Koan re-exports
`workgraph::witnessed` / `workgraph::scheduler` from its crate root — but
it is consumable only in-tree: `workgraph/Cargo.toml` carries
`publish = false` and no description / license / repository metadata,
there is no crate README or embedder-facing walkthrough beyond per-module
rustdoc, and the consumer API's identifiers are still working names
([design/scheduler-library.md](../../design/scheduler-library.md) marks
its type names *(working name)* — shapes are the commitment, identifiers
are not).

**Acceptance criteria.**

- The consumer-API identifiers are settled:
  [design/scheduler-library.md](../../design/scheduler-library.md) carries
  no *(working name)* markers, and the names it states are the exported
  ones.
- `workgraph` carries an embedder-facing crate README — workload
  instantiation, regions and carriers, the consumer API — with a minimal
  example embedder that compiles (doc test or `examples/`).
- Every public item in `workgraph` has rustdoc: the crate is clean under
  `missing_docs`.
- The crate packages for external use: publish metadata (description,
  license, repository) is present and `cargo publish --dry-run` succeeds.

**Directions.**

- *Distribution channel — open.* (a) Publish to crates.io (fixes the
  `workgraph` name, subject to availability); (b) consume as a git
  dependency from this repo, deferring the name claim. Actually pushing to
  crates.io is a user call at ship time either way.
- *Example embedder scope — open.* (a) A doctest-sized toy workload inside
  the crate README; (b) an `examples/` mini-embedder exercising the
  scheduler and regions end to end. Recommended: (a) for the README plus
  (b) only if the README example cannot stay honest at doctest size.

## Dependencies

API stabilization gates this item: any boundary-moving item lands first so
the surface being documented and frozen is the final one.

**Requires:**

- [Regions wholesale: the at-will allocation surface](regions-wholesale.md)
  — the last boundary move that reshapes the library's public surface.

**Unblocks:** none tracked — the project's terminal item.
