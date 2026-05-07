# Module system

Koan's abstraction unit is the *module*: a bundle of types and operations behind
a signature, with first-class module values and modular implicits providing
ergonomic generic dispatch. The module language, ascription primitive, and
first-class module values are shipped (stage 1); functors, axioms, and
implicits land in subsequent stages — see the [open-work index](#open-work) at
the bottom.
The doc lives in `design/` because modules are a cross-cutting language
concern that several roadmap items share, and capturing the whole shape in
one place keeps the staged work coherent.

The motivation is uniformity: multi-parameter dispatch, higher-kinded
abstraction, and representation hiding all fall out of one mechanism rather
than sitting in three.

## Structures and signatures

A **structure** (declared with `MODULE`) bundles type definitions, values,
and functions:

```
MODULE IntOrd = ((LET Type = Number) (LET compare = (FN ...)))
```

A **signature** (declared with `SIG`) is a module type — an interface
specifying what a structure must contain:

```
SIG OrderedSig = ((LET Type = Number) (LET compare = (FN ...)))
```

Module and signature names use the **Type-token** spelling: first character
ASCII-uppercase plus at least one lowercase character (`IntOrd`, `OrderedSig`,
`MakeSet`). Abstract types declared inside a signature use the same shape —
the convention is `Type` for the principal abstract type, with additional
abstract types named `Elt`, `Key`, `Val`, etc. when more than one is needed.
A bare `LET <TypeName> = <expr>` inside a signature body declares an abstract
type slot rather than a value binding. The token-class rule that distinguishes
`MODULE` (keyword: ≥2 uppercase, no lowercase) from `IntOrd` (Type token:
uppercase-leading with at least one lowercase) is described in
[type-system.md](type-system.md#token-classes--the-parser-level-foundation).

Structures can be **ascribed** to signatures via two operators that differ
only by a whitespace gap in the visual rendering, expressing "you can see
through this":

```
LET IntOrdView     = (IntOrd :! OrderedSig)   -- transparent
LET IntOrdAbstract = (IntOrd :| OrderedSig)   -- opaque
```

*Transparent ascription* (`:!`) checks that the structure satisfies the
signature but leaves type definitions visible: `IntOrdView.Type` resolves to
`Number` just as `IntOrd.Type` does. *Opaque ascription* (`:|`) additionally
hides the representation: outside the ascription, `IntOrdAbstract.Type` is
**not** the same type as `Number`, even though that's its underlying
definition. Type checking forbids passing an `IntOrdAbstract.Type` value to
anything expecting a `Number` — the abstraction barrier is enforced.

Opaque ascription is **generative**: each application mints a fresh
`KType::ModuleType { scope_id, name }` per declared abstract type. Two
distinct opaque ascriptions of the same source module yield distinct types
that cannot be confused. The carrier lives in
[`KType`](../src/dispatch/types/ktype.rs); the operators are registered as
ordinary builtins in [`ascribe.rs`](../src/dispatch/builtins/ascribe.rs).

Opaque ascription is the type-abstraction primitive. It replaces the
newtype-with-private-fields pattern that a trait system would need.

## Functors

A **functor** is a module parameterized by another module — a function from
modules to modules. Since modules are first-class values, functors are
ordinary FNs whose parameters are signature-typed and whose body returns a
`MODULE` expression:

```
LET MakeSet = (FN (MAKESET E: OrderedSig) -> SetSig = (
  MODULE Result = (
    (LET Type = ...)
    (LET insert = (FN (INSERT s: Type x: E.Type) -> Type = ...))
    ...
  )
))

LET IntSet = (MAKESET IntOrd)
```

`MODULE Name = (...)` is itself an expression: it both binds `Name` in the
enclosing per-call scope and evaluates to the module value, so the functor
body needs no separate "anonymous structure" form. The bound name (`Result`
above) lives only inside the call frame.

Functor application is **generative**: each call evaluates the body afresh,
and any inner `:|` mints fresh `KType::ModuleType` slots. `(MAKESET IntOrd)`
applied twice yields two distinct `Set` types that cannot be confused.
Generativity is a consequence of `:|`-per-call, not a separate mechanism.

Sharing constraints — pinning a functor's output abstract type to its input
— ride on the named-slot syntax for parameterized type expressions described
in [Parameterized type expressions](#parameterized-type-expressions). A
functor whose return type is `SetSig<Elt: E.Type>` declares the constraint
at the FN's return slot. There is no separate `with type` keyword.

Multi-argument functors are ordinary multi-parameter FNs. Currying is just
nested FNs.

## First-class modules

Modules are values: `KObject::KModule` flows through `LET`, ATTR, and
function calls like any other value. There is no separate pack/unpack form,
no `(module M)` construction syntax, and no `(val m)` projection. A module
named in expression position evaluates to its value, and `m.compare` is
ordinary attribute access.

Module-typed bindings reuse the existing ascription operators:

```
LET m = (IntOrd :! OrderedSig)   -- transparent: m.Type ≡ Number
LET m = (IntOrd :| OrderedSig)   -- opaque:      m.Type is fresh
```

`:!` and `:|` are the typing primitives. There is no third
`LET m: OrderedSig = IntOrd` form — it would express only the transparent
case and would be strictly less expressive than the operators that already
exist.

FN parameters and return types accept signature names directly. The
constrained-signature case (`OrderedSig<Type: Number>`) uses the named-slot
machinery in [Parameterized type expressions](#parameterized-type-expressions).

## Parameterized type expressions

The `<>` machinery shipped for `List<T>`, `Dict<K, V>`, and
`Function<(args) -> R>` extends to carry sharing constraints, signature
constraints on implicit-parameter types, and witness-typed instantiations.
Three extensions to the shipped surface
([type-system.md](type-system.md#container-type-parameterization)):

- **Named slots.** `OrderedSig<Type: Number>` pins the abstract `Type` slot
  of a signature to `Number`. `Set<Elt: Number, Ord: IntOrd>` does the same
  for a parameterized type constructor with multiple slots. Named binding
  uses the same `name: value` shape FN parameters use, just inside `<>`.
  Positional fill is accepted when slot order is unambiguous.
- **Type-valued expressions.** `<>` slots accept any expression that
  evaluates to a `KType` or `KModule`, not only bare type-name tokens.
  `List<M.Type>` is an ATTR access yielding the abstract type of module
  `M`. The slot's declared kind decides what the engine expects.
- **Module-kind slots.** Type constructors can declare slots that take
  modules. `Set<Elt: Number, Ord: IntOrd>` works because `Set`'s second
  slot is declared `OrderedSig`-kind. Distinct module values bound to the
  same slot give distinct concrete types — the mechanism behind witness
  types in stage 7.

Sharing constraints, modular-implicit signature constraints, and witness
types share this one notation. The implicit *marker* itself (which
parameter is implicit) is orthogonal — see
[Modular implicits](#modular-implicits).

## Modular implicits

A function can declare an **implicit module parameter**:

```
LET sort = (FN (SORT xs: List<M.Type> {M: OrderedSig}) -> List<M.Type> = (...))
```

At a call site `(SORT [3, 1, 2])`, the compiler infers `M.Type = Number`,
searches in scope for a module satisfying `OrderedSig<Type: Number>`, and
inserts it. Searching is **lexical**: the candidate set is the implicit
modules defined in the current module plus those explicitly imported.
Nothing leaks through transitive dependencies.

Specificity is the standard rule: more-specific candidates beat
less-specific ones; ties between unrelated candidates are an error. The
disambiguation primitive when ambiguity arises is **explicit module
application** at the call site — the user names which candidate to use.

Two surface forms remain TBD and are decided alongside the stage-5
implementation: the implicit-parameter marker (`{...}` is a placeholder
above) and the **explicit-application form** — the lowest-level surface
for naming a candidate at the call site to break an ambiguity. No
strawman has been picked; examples below and in the
[equivalence-checking section](#cross-implicit-equivalence-checking) that
read OCaml-shaped (`with type t = ...`, `module type ... = sig ... end`,
`observation ... via ...`) are pre-Koan placeholders awaiting design.
The constraint half — the signature expression to the right of `:` —
uses the named-slot machinery described in
[Parameterized type expressions](#parameterized-type-expressions). Sugar
(block-scoped binding, module priority, selective imports) lands later,
after enough real code has been written to know which patterns are
common. See
[the syntax-tuning stage](../roadmap/module-system-7-syntax-tuning.md).

### Higher-order restriction

**Implicit modules cannot themselves take implicit parameters.** A normal
functor can take implicit module arguments at its application site, but the
*resolution* never recurses through implicit search to satisfy an implicit
parameter of a candidate module. This cuts the worst tangle in the
modular-implicits design space — search staying one level deep keeps both
inference and resolution decidable in the simple case, and avoids the
exploding-search-tree pathology that has historically blocked OCaml's
modular-implicits proposal.

## Axioms and property testing

A signature can carry **axioms** — propositional contracts on its
operations:

```
SIG OrderedSig = (
  (LET Type = ...)
  (LET compare = (FN (COMPARE x: Type y: Type) -> Number = ...))
  (LET gen = (FN (GEN r: Random) -> Type = ...))

  (AXIOM #((compare x x) = 0))
  (AXIOM #((sign (compare x y)) = (- (sign (compare y x)))))
  (AXIOM #(IMPLIES (and (<= (compare x y) 0) (<= (compare y z) 0))
                   (<= (compare x z) 0)))
)
```

Each `AXIOM` carries a quoted bool expression — the `#(...)` sigil produces
a `KExpression` value. At ascription time, the engine evaluates each
axiom's quote under a scope it builds by drawing samples from the module's
`gen` slot for every free identifier the quote references. Variable types
are resolved through the surrounding signature scope: `x`, `y`, `z` above
take type `Type` because `compare`'s parameters fix that kind.

`IMPLIES` is the engine's discard combinator — when the antecedent is
false, the sample is dropped without counting against the test budget,
matching standard property-based-testing practice for conditional axioms.

When a structure ascribes a signature with axioms, the engine runs each
axiom against random samples. Failures are ascription errors with a
reported counterexample (and shrunk to a minimal case where the engine
permits). This catches *invalid* implementations mechanically —
non-transitive comparisons, hashes that disagree with their own equality,
monoids whose identity isn't.

**Generators live in modules; the signature requires them.** A
`LET gen = (FN ...)` slot in a signature body is an obligation: every
ascribing module must supply a generator for the abstract type. This folds
generator presence into the existing structural-conformance check —
ascription of a module without a `gen` slot fails with the same
"missing field" error as any other unsupplied operation. There is no
sidecar generator registry.

**Generators compose through functor application.** A functor body that
constructs the result module's `gen` from the parameter's `gen` —
`MakeSet(E)`'s `gen` builds set samples by drawing from `E.gen` — gives
the composed module its generator mechanically. Composition is a
module-language property; the engine just calls `M.gen` like any other
operation.

The **property-testing engine** is a Rust-side subsystem of the compiler,
deliberately disjoint from the dispatcher and the scheduler. Two reasons
for the separation: (a) the engine is reusable as a general testing tool
against ordinary Koan code, not only against signature axioms; (b) keeping
it out of the dispatcher and scheduler keeps both simple. The engine's job
is to take a quoted axiom, generate samples for the variables it binds via
the module's `gen`, evaluate the quote, and report counterexamples;
nothing in the engine knows about modules or implicits per se.

**Randomness threads as a monadic effect.** Generators take a `Random`
parameter rather than consuming raw entropy ambiently — see
[design/effects.md](effects.md). Until stage 5 lands modular implicits,
the engine threads the random module through generator calls explicitly.
This is why stage 4 depends on the in-language monadic-effect surface.

## Cross-implicit equivalence checking

When implicit search finds multiple candidates that all satisfy the same
query, the compiler runs a **behavioral equivalence test** between them
using the property-testing engine:

```
For every pair (M, N) of in-scope candidates for ORDERED with type t = T:
  for sampled x, y from T's generator:
    assert M.compare(x, y) == N.compare(x, y)
```

If candidates agree on all sampled inputs, ambiguity is silent — pick either,
they're observably the same. If they disagree, the search fails with a
counterexample-bearing error:

```
error: ambiguous implicit ORDERED with type t = Number — and the candidates disagree
  IntOrd.compare(5, 3) = -1
  IntOrdReverse.compare(5, 3) = +1
  these modules are not behaviorally equivalent; pick one explicitly
```

This is the **coherence story**. It is probabilistic, not a proof — a
sufficiently adversarial pair of modules that agree on the sampled
distribution but disagree elsewhere will pass. For the common bug shapes
(reversed orderings, off-by-one comparisons, different hash seeds) the
disagreement is dense and the test catches it on the first sample. For
signatures where the operation deliberately under-specifies the return value
(`compare` where only the *sign* matters), a signature can declare an
**observation** that the equivalence check uses instead of direct value
equality:

```
module type ORDERED = sig
  ...
  observation compare via sign
end
```

When stronger guarantees are required, **witness types** (an opt-in feature
in the syntax-tuning stage) reflect the implicit's identity in the abstract
type itself — `Set<Number, IntOrd>` and `Set<Number, IntOrdReverse>` become
distinct types that cannot mix. Ergonomic but verbose; a tool for the cases
where probabilistic coherence isn't enough.

The OCaml-shaped fragments in this section (the `with type t = ...`
ambiguity-error message and the `module type ORDERED = sig ... observation
compare via sign end` observation declaration) are placeholders. The
explicit-disambiguation form, the ambiguity-error rendering, and the
observation-declaration syntax are all decided alongside the stage-5 and
stage-6 implementations — see
[the syntax-tuning stage](../roadmap/module-system-7-syntax-tuning.md).

## Compile-time scheduling

Type inference and implicit search interleave: an implicit search needs the
constrained types resolved before it can run, and the resolved implicit may
in turn refine type variables that other inferences are waiting on. Both are
modeled as **node types in the existing scheduler**, alongside the runtime
`Dispatch` node described in [execution-model.md](execution-model.md):

- `Infer(expr, ctx)` — infers a type, may spawn sub-`Infer` nodes for
  sub-expressions and `ImplicitSearch` nodes for implicit parameters.
- `ImplicitSearch(sig, types, scope)` — finds an implicit module, depends on
  `Infer` nodes for its constrained types, may itself refine type variables.

This is the **weak metacircular** form: the same scheduler engine that runs
Koan value-language work runs the type checker. The Rust code for `Infer` and
`ImplicitSearch` node bodies is distinct from `Dispatch`'s, but the
scheduling, dependency tracking, and cycle detection are shared. The strong
form (compile-time tasks written in Koan and executed by the scheduler) is
not a goal; the architecture leaves the door open without paying its
bootstrapping cost.

What this buys:

- **Cycle detection is uniform.** A cycle in implicit resolution and a
  runtime infinite loop are the same kind of bug to the scheduler.
- **Topological ordering falls out.** "When is enough inference done to run
  search?" is just "when this search task's dependencies have completed."
- **Incremental compilation is a side effect.** If the scheduler memoizes
  task results (a separate decision), recompilation only re-runs tasks
  whose dependencies changed.

What it requires the scheduler to grow into:

- **Multi-target unification.** A single inference task may refine many type
  variables that downstream tasks are waiting on. Either thread a shared
  substitution out-of-band, or model type variables as their own nodes that
  get refined and woken up.
- **Phase boundary.** Type-checking must complete before evaluation begins
  for a compilation unit. Whether this is one batch boundary or finer-grained
  per-definition phase tracking is a design choice for stage 1.
- **Failure isolation.** When an inference or search fails, dependents fail
  too — but independent subtrees should still finish so the user sees
  multiple errors per compile rather than one-at-a-time.

## Resolution and coherence: the design dials

| Dial | Setting |
|---|---|
| Implicit search scope | Lexical + explicitly imported only. No transitive leak. |
| Specificity | Most-specific-wins. Unrelated ties are errors. |
| Ambiguity policy | Strict — error unless candidates pass cross-equivalence. |
| Coherence | By convention, with property-tested equivalence as a safety net. Witness types opt-in for stronger guarantees. |
| Orphan rule | Soft (lint-only). Implicits should live with their signature or with one of their dispatched types; deviating warns but doesn't error. |
| Higher-order implicits | Disallowed. Implicit modules cannot take implicit parameters. |
| Disambiguation primitive | Explicit module application; surface syntax TBD, decided alongside the stage-5 implementation. |

## Properties of this design

- **Multi-parameter dispatch is native.** A signature can declare multiple
  abstract types; implicit search dispatches on all of them, so binary-operator
  dispatch (`+`, `==`, `intersect`) and other multi-type predicates have a
  uniform mechanism rather than a partial-order tiebreak.
- **Higher-kinded abstraction is native.** Signatures can declare type
  constructors (`type 'a t`); functors can take and return them.
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
seven-stage plan is structured so each stage produces a usable end state,
absorbing the conceptual cost incrementally.

## Open work

Implementation stages remain, each producing a usable end state. (Stage 1
— the module language itself: `MODULE`, `SIG`, `:|`, `:!`, and per-module type
identity via `KType::ModuleType` — shipped and is described in the body
above. First-class module values shipped alongside it: `KObject::KModule`
flows through `LET` and ATTR like any other value, so a separate pack/unpack
construct isn't needed; the remaining first-class-modules work folds into
later stages — signature-bound dispatch (modules-as-values typed against a
specific signature) is part of stage 5, and the static-signature-at-use-site
obligation for the type checker is part of stage 1.5.)

- [Stage 1.5 — Scheduler integration](../roadmap/module-system-1.5-scheduler.md)
  — `Infer` and `ImplicitSearch` scheduler nodes, the type-checking phase
  boundary, and multi-target unification. Re-runs the
  [memory-model audit slate](memory-model.md#verification) against
  the post-stage-1 runtime plus the new scheduler nodes.
- [Stage 2 — Functors](../roadmap/module-system-2-functors.md) — parametric
  modules with explicit application and sharing constraints. Ships generic
  data structures.
- [Stage 4 — Property testing and axioms](../roadmap/module-system-4-axioms-and-generators.md)
  — Rust-side property-testing engine kept disjoint from dispatch; axiom
  syntax in signatures (`AXIOM #(...)` over quoted bool predicates);
  generators-as-required-signature-slots; compile-time axiom checking on
  ascription. Generators thread randomness via the monadic effect surface
  ([design/effects.md](effects.md)), so this stage requires
  [monadic-side-effects](../roadmap/monadic-side-effects.md). Independent
  of stage 5's implicit dispatch.
- [Stage 5 — Modular implicits](../roadmap/module-system-5-modular-implicits.md)
  — implicit module parameters, lexical resolution, strict-on-ambiguity
  policy, explicit-application disambiguation. The "real" generic-code
  ergonomics arrive here.
- [Stage 6 — Equivalence-checked coherence](../roadmap/module-system-6-equivalence-checking.md)
  — cross-implicit equivalence testing using the stage-4 engine. The
  coherence story.
- [Stage 7 — Syntax tuning and witness types](../roadmap/module-system-7-syntax-tuning.md)
  — disambiguation sugar designed against patterns from real stage-5 code,
  plus opt-in witness types for stronger-than-probabilistic coherence.
