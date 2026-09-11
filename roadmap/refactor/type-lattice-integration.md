# Integrate the type lattice

Wire the core of [type-lattice.md](../../design/typing/type-lattice.md) into the machine, re-home
the glue, and delete the shipped lattice.

**Problem.** [types/](../../src/machine/model/types.rs) is both the lattice and the glue around
it: admission predicates that read values and parser parts, schema projection that reads `Scope`
and `ModuleDraft`, window elaboration from the AST, region-brand token storage, and the dispatch
picker's specificity fold all sit beside the node type and its relations, and every caller in the
machine reaches types through that directory. [operators.rs](../../src/machine/model/operators.rs)
defines the reduction modes a signature's identity is digested over.
[type_lattice/](../../src/type_lattice.rs) stands beside all of it with no caller, and gains one
only as each of these is moved to its own owner and pointed at the core.

**Acceptance criteria.**

- Every caller outside `src/type_lattice` reaches types through the core's public relations; no
  second implementation of any of them exists in the tree.
- `matches_value`, `matches_held`, `matches_type`, `accepts_carried`, `accepts_working_part`,
  `accepts_part` and `slot_ktype` live with the values and parts they inspect and call the core's
  type-level relations; the channel preference between a token capture and a resolved value is
  stated there, not in the lattice.
- `project_decl` and `raw_self_sig` live with the callers that hold the scope and draft they
  project; `project_decl` re-sources own-member references to `ScopeId::SENTINEL` and hands the
  core a finished schema.
- The coercion tables and the bucket-key probe live with ascription and dispatch, built over the
  core's substitution, `signature` and node-read doors.
- Region-brand storage of dispatch tokens lives in `src/memory`.
- `operators.rs` imports `ReductionMode` and `FoldDirection` from the core; one definition exists.
- The live-callable picker ranks through the core's `shape_specificity`.
- Every definition that mints a shape translates its declaration-order quantifier bindings
  through the renumbering `shape_type` reports.
- The shipped node, registry, digest and relation files under `machine::model::types` are deleted,
  along with every hand-written test a law now covers.
- Every dispatch verdict that changes is enumerated, each with the law the old verdict broke, and a
  test pins the new verdict. The enumeration covers at least: container element inference yielding
  unions where it yielded `Any`; overloads that the tier rules ranked and the order leaves
  ambiguous; quantified admission that no longer depends on argument order and no longer accepts a
  mixed-type pair.
- A test pins that a rendered type parses back to the same handle, over the core's generated
  types.
- A benchmark over a signature-heavy program shows no regression attributable to the materialized
  substitution; a "references any bound member" guard survives only if measured to pay for itself.
- The acceptance criteria of [one structural walk](type-structure-combinator.md) and
  [substitute, then ask](substitution-walk-collapse.md) hold over the tree.
- The full `cargo test`, the Miri slate, and the seam-equivalence check pass.

**Directions.**

- *Verdict changes — decided.* Where the property suite shows a shipped verdict violated a law, the
  law wins; the change is enumerated, not silently absorbed.
- *Substitution fast path — open.* Measure first: a rebuild of a member-free type returns the input
  handle, so the guard may buy nothing over content addressing.
- *Order of re-homing — open.* Whether admission, projection and storage move in one change or one
  each. Recommended: one each, with the core's relations called through the old paths in between.
- *Quantified values on the value lane — deferred.* The value lane erases a quantified callable to
  its bounds, so a polymorphic slot type is not satisfied by a polymorphic value even though the
  core relates the shape types by instantiation. Carrying quantified shapes on the value lane is a
  follow-up written once the core is wired in.
- *Bound syntax — deferred.* The core carries a bound on every rigid variable; `FOR ALL (Elt
  :Number)` and `TYPE Elt :Number` are builtins work written once the core is wired in.

## Dependencies

The two subsumed items retire with this one: their criteria are stated over the shipped files and
are met when those files are gone.

**Requires:** none — the lattice core is in the tree.

**Unblocks:** none tracked yet.
