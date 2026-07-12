# Type and module system

Koan's type system and module system together — they share the same
scheduler-driven elaborator and the same nominal-identity carrier, so the
docs live in one subdirectory. [open-work.md](open-work.md) carries the
work that remains.

The motivation is uniformity: multi-parameter dispatch, higher-kinded
abstraction, and representation hiding all fall out of one mechanism rather
than sitting in three.

## Where to look

Type-system mechanics:

- [tokens.md](tokens.md) — the parser-level Keyword / Type / Identifier
  split that lets type names occupy a syntactic slot without quoting.
- [ktype/](ktype/README.md) — `KType` variants, container parameterization,
  variance, type-position slot kinds, function signatures, dispatch and
  slot-specificity, and the limitations the static-typing work will close.
- [elaboration.md](elaboration.md) — how a type name resolves to a
  `KType`: the scheduler-driven elaborator, recursion via threaded-set
  recognition, module-qualified names, the binding-map partition that
  separates type-name lookups from value-name lookups, the
  `KType::Unresolved` bare-leaf transient, and the two-layer
  resolution memo that amortizes elaboration cost.
- [user-types.md](user-types.md) — the `RecursiveSet` nominal model: a
  member's `KType::SetRef` is the per-declaration identity for NEWTYPE, named
  UNION, MODULE, opaque ascription, and NEWTYPE, with `SetLocal` siblings and
  the `RecursiveGroup` handle. Covers specificity stratification with the
  `OfKind(KKind)` family-kind slot, finalize-time type-only install through
  `Scope::register_type_upsert`, the `RECURSIVE TYPES` block for mutually
  recursive nominals, and the `NEWTYPE` keyword's `Wrapped` carrier with its
  newtype-over-newtype collapse invariant encoded in the field type.
- [lookup-protocol.md](lookup-protocol.md) — the three-layer foundation
  every dispatch and name-resolution site threads: `Scope` chain-walks
  ancestors, `Bindings` finds entries gated by the visibility predicate,
  `KType` predicates admit or reject the candidate. Names the entry
  points and explains why this is a foundation (correctly distributed)
  rather than a seam.

Module-system mechanics:

- [modules.md](modules.md) — structures (`MODULE`), signatures (`SIG`),
  the transparent and opaque ascription operators (`:!` and `:|`), and
  first-class module values flowing through `LET`, ATTR, and function
  calls.
- [functors.md](functors.md) — modules parameterized by modules: surface
  vs machine semantics, per-call generativity, deferred return types,
  higher-kinded type-constructor slots, and the `WITH` infix
  builtin family for sharing constraints and witness-typed
  instantiations.
- [implicits.md](implicits.md) — implicit module parameters, lexical
  resolution, axioms with property-tested checking, cross-implicit
  equivalence checking, and the resolution-and-coherence design dials.
- [generics.md](generics.md) — generic functions as functors over their
  type parameters, selected and applied by implicit resolution; the
  parametric vs operation-bearing split, type-argument-from-carried-type
  versus module-argument-from-search, and dependent parameters.
- [scheduler.md](scheduler.md) — type inference and implicit search as
  ordinary `Dispatch` / dep-finish scheduler work, with no parallel
  `Infer` / `ImplicitSearch` node-kind track.
- [type-language-via-dispatch.md](type-language-via-dispatch.md) — the
  sigil `:(...)` as a parse-context marker; parameterized type
  construction (`LIST`, `MAP`, `FN`, `Functor`) and user-defined
  functor application registered as keyworded overloads sharing the
  value-side candidate-bucket and binder-admission machinery.

[open-work.md](open-work.md) carries the roadmap pointers for the
module-system stages plus the cross-cutting standard-library,
group-operators, and JIT items.

## Properties of this design

- **Multi-abstract-type implicit resolution is native.** A signature can
  declare multiple abstract types; implicit search aligns all of them against
  the call site's argument types simultaneously, so binary-operator dispatch
  (`+`, `==`, `intersect`) and other multi-type predicates pick the right
  implicit through one mechanism rather than ranking single-type candidates
  against each other. (Multi-parameter dispatch on declared types is already
  native to FN; this property is about the implicit-search layer on top.)
- **Higher-kinded abstraction is native.** Signatures can declare type
  constructors (`TYPE (Type AS Wrap)`); functors can take and return them.
- **Representation hiding is principled.** Opaque ascription is the
  abstraction barrier — privacy is an outcome of the type system rather than
  a separate visibility mechanism.
- **Coherence is scoped, not global.** Two libraries can ship different
  implicits for the same signature and types, coexisting in the program as
  long as they don't meet at a call site. Property-tested equivalence catches
  the cases where they do meet and disagree. A soft lint replaces the global
  orphan rule a strict trait system would need.
- **Versioning is natural.** Different modules can hold different
  implementations of the same abstraction; users select by import.

The cost is a larger conceptual surface — a module language layered over the
value language — and a more sophisticated implicit-resolution algorithm. The
roadmap in [open-work.md](open-work.md) is partitioned so each item produces
a usable end state.
