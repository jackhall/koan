# Codebase-wide naming and responsibility audit

A single deliberate pass over all of `src/**` to reconcile concept names with
current behavior and merge responsibilities that have drifted into two places.

**Problem.** Several core concepts have been revised many times — among them the
dispatcher, dispatch's extension to types, the type-language collapse, the
`TypeExpr`→`TypeName` carrier collapse, and the `function`→`ReturnContract`
generalization. A revision renames or re-homes the symbols at its center but not
always their neighbors, so three drift modes accumulate: a name can describe a
former role rather than what the code now does; two distinct concepts can carry
confusingly similar names (or one name can cover two concepts); and the same
responsibility can live in two places under different names. Nothing has reconciled
the vocabulary across the whole tree — drift is noticed incidentally, per-PR, in
whatever file a change happens to touch, never as a systematic sweep. The
accumulation predates the recent refactors and is not confined to the
recently-churned subsystems.

**Impact.**

- *Names can be trusted.* Every concept-level name (type, variant, field, public
  method, module) describes its current behavior, so a reader navigates by name
  without cross-checking the implementation.
- *Distinct concepts read as distinct.* Overlapping name families are
  disambiguated, removing the second-guessing where two similar names mean
  different things.
- *One responsibility, one owner.* Logic that had drifted into two symbols under
  different names is merged, shrinking the surface a future change must keep in
  sync.
- *Source and design docs share one vocabulary*, lowering ramp-up cost and keeping
  the design tree's terms aligned with the code.

**Directions.**

- *Whole-tree scope — decided.* The audit covers all of `src/**`, not only the
  recently-churned subsystems; the drift it targets is older and broader than the
  latest refactors.
- *Three candidate categories — decided.* Every finding is classified as (1) a
  stale/misleading name, (2) an overlapping/ambiguous name, or (3) duplicated
  responsibility, and carries file:line evidence of the name-vs-behavior or
  symbol-vs-symbol mismatch. Concept-level only — types, variants, fields, public
  methods, modules — not private locals.
- *Method — open.* Lean on the `rust-abstraction` skill (file-level seams),
  `doc-abstraction` (concepts that span docs/source without one owner), and
  `modgraph` (complexity scoring of a proposed reshuffle) to locate duplication,
  and the SCIP-driven `modgraph_rewrite.py item` path to scope extractions.
  Recommended: one read-only sweep per top-level area (`src/machine/model`,
  `.../execute`, `.../core`, `src/builtins`), each producing a candidate table,
  then a consolidation pass that de-duplicates findings across areas.
- *Output packaging — open.* Whether the audit lands one consolidated rename/merge
  plan or spawns a follow-up roadmap item per subsystem. Recommended: produce the
  consolidated candidate list first, then split execution by blast radius — a
  crate-wide rename is a different risk profile from a local one.
- *Execution and validation — decided.* Renames go through the `rust-refactor`
  skill (`ast-grep` structural rewrites, `cargo build` as the gate); every renamed
  symbol named in a `//` comment or design doc is swept with the documentation
  skill's `doclinks` pass so source and docs stay in step.

## Dependencies

**Requires:** none — an audit depends on nothing.

**Unblocks:** none tracked yet — the audit is expected to spawn its own follow-up
rename/merge items as it surfaces concrete candidates.

The in-flight type-representation items (tagged-union variants as types, and
plain-English type-operation surfaces) will themselves rename and reshape their own
areas. Sequence the audit's
passes over those areas after those items land, or coordinate, so the same region
is not audited twice.
