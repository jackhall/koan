# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
[roadmap/](roadmap/).

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](design/) — six
topical docs covering the execution model, memory model, functional programming, type
system, expressions and parsing, and error handling. What's shipped so far: user-defined
functions, the dispatch-as-node scheduler refactor, first-cut tail-call optimization, the
leak fix (with lexical closures + per-call arenas), structured error propagation, the
user-defined-types substrate (return-type enforcement at runtime), the IF-THEN→MATCH
consolidation (`MATCH` accepts `Bool` directly via projection at entry), per-parameter
type annotations on user-fn signatures, container type parameterization (`List<T>`,
`Dict<K, V>`, `Function<(args) -> R>`), transient-node reclamation (Bind/Aggregate
sub-trees recycled via a per-slot deps sidecar + free-list, keeping repeated-call
scheduler memory near-constant), per-call-frame chaining for builtin-built frames
(MATCH's child-scope `outer` no longer dangles when a TCO replace drops the
call-site frame), and a targeted KFuture lift anchor (an addresses-only side-table
on `RuntimeArena` answers a precise membership query, replacing the previous
always-anchor conservative path). The next signature revision after error handling
lands monadic side-effect capture; the remaining type/trait sequence (methods, traits,
trait inheritance) unlocks the items downstream (group-based operators), so it sits in
the middle of the sequence rather than last.

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — `Scope::out` is one ad-hoc effect channel; every future effect (IO, time, randomness)
  needs a uniform carrier. (Previously a soft prerequisite of transient-node reclamation;
  now decoupled — reclamation shipped without touching `BuiltinFn`.)
- [Per-type identity for structs and methods](roadmap/per-type-identity.md) — every user
  struct collapses to `KType::Struct`; methods can't attach to specific types. Next slice
  of the type/trait sequence after container type parameterization.
- [Quote and eval sigils](roadmap/quote-and-eval-sigils.md) — no surface form to
  force-evaluate a metaexpression or suppress evaluation inside a dict/list literal.
- [Other deferred surface items](roadmap/deferred-surface-items.md) — errors-as-values,
  catch-builtins, `RAISE`, source spans on `KExpression`, continue-on-error.
- [Refactor for cleaner abstractions](roadmap/refactoring.md) — standing/exploratory; act
  only when the next feature would multiply existing duplication.

## Open items

### Memory and runtime substrate

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — `Scope::out` is one ad-hoc effect channel; every future effect (IO, time, randomness)
  needs a uniform carrier.
- [Open issues from the leak-fix audit](roadmap/leak-fix-audit.md) — Miri hasn't run on the
  per-call-arena transmutes.

### Type system

- [Per-type identity for structs and methods](roadmap/per-type-identity.md) — every user
  struct collapses to `KType::Struct`; methods can't attach to specific types.
- [`TRAIT` builtin for structural typing](roadmap/traits.md) — no surface for "anything
  that can be iterated"; user code redoes per-concrete-type variants.
- [Trait inheritance](roadmap/trait-inheritance.md) — `Ord` extending `Eq` is the
  standard layering; trait hierarchies are flat without it.
- [Group-based operators](roadmap/group-based-operators.md) — `+`/`-` form a math group
  but the language treats every operator as a flat independent builtin.

### Surface and ergonomics

- [Quote and eval sigils](roadmap/quote-and-eval-sigils.md) — no surface form to
  force-evaluate a metaexpression or suppress evaluation inside a dict/list literal.
- [Module system and directory layout](roadmap/module-system.md) — a Koan codebase is one
  file; no import, no module path, no project-level entry point.
- [Other deferred surface items](roadmap/deferred-surface-items.md) — errors-as-values,
  catch-builtins, `RAISE`, source spans on `KExpression`, continue-on-error.

### Future-facing

- [Static type checking and JIT compilation](roadmap/static-typing-and-jit.md) — the
  tooling and performance ceiling; both want a phase between parse and execution.
- [Refactor for cleaner abstractions](roadmap/refactoring.md) — standing item: remove
  accidental abstraction when the next feature would multiply existing duplication.
