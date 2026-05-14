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
`KType::UserType { kind: Module, scope_id, name }` per declared abstract
type. Two distinct opaque ascriptions of the same source module yield
distinct `scope_id`s and therefore distinct types that cannot be confused.
The carrier lives in
[`KType`](../src/runtime/model/types/ktype.rs); the operators are registered as
ordinary builtins in [`ascribe.rs`](../src/runtime/builtins/ascribe.rs).

Opaque ascription is the type-abstraction primitive. It replaces the
newtype-with-private-fields pattern that a trait system would need.

## Functors

A **functor** is a module parameterized by another module — a function from
modules to modules. Koan presents this with two layered semantics:

- *Surface semantics* — modules are part of the **type language**. A
  signature-typed FN parameter (`Er: OrderedSig`) is a type-language
  binder, like an OCaml functor's parameter. `Er.Type` in a type-position
  slot is type-language projection — extracting the module's abstract
  type. Identifier-class names (`er`, `mo` — lowercase-first per
  [type-system.md](type-system.md#token-classes--the-parser-level-foundation))
  are value-language only and a hard error in any type-position slot.
- *Machine semantics* — modules are **first-class values**.
  `KObject::KModule` flows through the scheduler like any other value;
  functors are ordinary FNs whose parameters are signature-typed and whose
  body returns a `MODULE` expression.

The two readings rest on the same scheduler — there is no separate
type-checking pass, no parallel module language. The elaborator's
token-class-driven lookup is the seam: Type-class names in type-position
slots consult the type-language binders; identifier-class names do not.
The example below illustrates both readings — the surface reads `Er` as a
type-language binder, the machine sees a value parameter whose value is a
module:

```
LET MakeSet = (FN (MAKESET Er: OrderedSig) -> SetSig = (
  MODULE Result = (
    (LET Type = ...)
    (LET insert = (FN (INSERT s: Type x: Er.Type) -> Type = ...))
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
and any inner `:|` mints fresh `KType::UserType { kind: Module, .. }`
slots. `(MAKESET IntOrd)` applied twice yields two distinct `Set` types
that cannot be confused.
Generativity is a consequence of `:|`-per-call, not a separate mechanism.
The applicative variant — same-functor-applied-to-same-module producing the
same output types, so independent call sites resolving to the same implicit
module interoperate — is open work; see [Open work](#open-work).

Sharing constraints — pinning a functor's output abstract type to a
specific concrete type — ride on the `SIG_WITH` builtin described in
[Type expressions and constraints](#type-expressions-and-constraints). A
functor whose return type is `(SIG_WITH SetSig ((Elt: Number)))` declares
the constraint at the FN's return slot; the body's `MODULE Result`
must mirror `Elt = Number` for the return-type check to admit it. There
is no separate `with type` keyword.

Pin values that reference only the FN's outer scope are elaborated at
FN-construction time. Concrete builtins (`Number`, `Str`) and
outer-scope-bound type values (`(MODULE_TYPE_OF Mo Type)` where `Mo` is
bound outside the FN) both work as pin values resolved eagerly.

Pin values that reference a per-call FN parameter
(`(MODULE_TYPE_OF Er Type)` for an `Er` declared on the FN itself) are
*templated*: the unresolved `TypeExpr` is captured on the FN at
construction time. At each call's **dispatch boundary** — after
parameters bind, before the body runs — `substitute_params` rewrites the
parameter name in the captured expression and the substituted expression
is scheduled as a sub-Dispatch. The result is the call's concrete
return-type `KType`, known before the body produces its first value;
body and return-type elaboration proceed concurrently and join at the
outer Combine for the slot check. Elaborating the return at the dispatch
boundary — rather than waiting until the body returns — keeps
sharing-constraint pins meaningful as call-site contracts, parallelizes
return-type work with body work, and parallels how parameter-typed slots
already flow.

Multi-argument functors are ordinary multi-parameter FNs. Currying is just
nested FNs.

## Higher-kinded type slots

Signatures can declare **type-constructor slots** — abstract types that take
a type parameter — so parametric abstractions like the `Monad` signature in
[design/effects.md](effects.md) are expressible:

```
SIG Monad = (
  (LET Wrap = (TYPE_CONSTRUCTOR Type))
  (LET pure = (FN (PURE x: Number) -> Wrap<Number> = ...))
  (LET bind = (FN (BIND m: Wrap<Number> f: Function<(Number) -> Wrap<Number>>) -> Wrap<Number> = ...))
)
```

`(TYPE_CONSTRUCTOR <param>)` is the declaration form: inside a SIG body it
binds the slot name (`Wrap` above) to a template
`KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. }`
carrying the parameter symbol list. The builtin lives in
[`type_ops.rs`](../src/runtime/builtins/type_ops.rs).

Application uses the existing `<>` parameterization surface:
`Wrap<Number>` in a type-position slot elaborates through
[`elaborate_type_expr`](../src/runtime/model/types/resolver.rs)'s
constructor-application arm into
`KType::ConstructorApply { ctor: <the Wrap UserType>, args: [Number] }` —
structural identity by `(ctor, args)`, mirror of `List(_)` / `Dict(_, _)`.
The arm arity-checks against the constructor's `param_names.len()` and
parks on a placeholder when the outer name is an in-flight `LET`, the same
forward-reference path bare-leaf type names use.

Higher-kinded slots are **per-call generative on the same path as ordinary
abstract type slots**. Two opaque ascriptions of the same source module
against the same SIG mint distinct `TypeConstructor` carriers under each
resulting module's `type_members[Wrap]` — their `(scope_id, name)` pairs
differ, so `First.Wrap<Number>` and `Second.Wrap<Number>` are incomparable
types. The minting site is the same loop in `ascribe.rs:body_opaque` that
mints `kind: Module` slots; it inspects the SIG's
`bindings.types[<slot>]` and matches `UserTypeKind::TypeConstructor` so the
slot inherits its declared kind.

Stage 2 ships **arity-1 only.** The `param_names` list always carries one
entry; multi-parameter constructors (`Functor F G`) are deferred. The
parameter symbol must be a Type-classified token (≥1 lowercase character):
the parser rejects single-letter capitals (`T`, `E`) at lex time, so
surface forms in this doc using `T` are conceptual — real code writes
`(TYPE_CONSTRUCTOR Type)` or `(TYPE_CONSTRUCTOR Elt)`. The
[token-class rule](type-system.md#token-classes--the-parser-level-foundation)
is the parser-level cause.

`ConstructorApply` has no value-level runtime carrier in stage 2: no
`KObject` reports a `ConstructorApply` `ktype()`. The variant flows through
the type-position machinery (FN return-type elaboration, signature-body
ascription) but the corresponding value-level admissibility — wrapping a
concrete value in `Wrap<Number>` and unwrapping it — is a stage-3 concern.
Cross-module application (`M.Wrap<Number>` reached via ATTR-then-apply)
isn't exercised end-to-end yet; bare `Wrap<T>` in a signature body or
against a root-scope-bound constructor is the path the stage-2 tests pin.

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
constrained-signature case (`(SIG_WITH OrderedSig ((Type: Number)))`)
uses the `SIG_WITH` builtin in
[Type expressions and constraints](#type-expressions-and-constraints).

## Type expressions and constraints

The `<>` parameterization shipped for `List<T>`, `Dict<K, V>`, and
`Function<(args) -> R>`
([type-system.md](type-system.md#container-type-parameterization))
covers positional structural types. Sharing constraints,
modular-implicit signature constraints, and witness-typed
instantiations ride on a separate **parens-form builtin family** that
reuses the `name: value` triple shape FN parameters and STRUCT fields
use. The two surfaces stay disjoint: `<>` for structural shapes whose
slot semantics are positional, parens-form builtins for slot-named
constraints.

- **`SIG_WITH`.** Pins abstract type slots of a signature to specific
  concrete types. `(SIG_WITH OrderedSig ((Type: Number)))` is
  `OrderedSig` with its `Type` slot pinned to `Number`;
  `(SIG_WITH Set ((Elt: Number) (Ord: IntOrd)))` pins multiple slots in
  one call. The inner parens groups are each one `name: value` triple,
  matching the shape FN parameters parse.
- **Type-valued slot values.** `SIG_WITH` slot values accept any
  expression that evaluates to a `KType` or `KModule`, not only bare
  type-name tokens. `(SIG_WITH MySig ((Elt: (MODULE_TYPE_OF Mo Type))))`
  works because `MODULE_TYPE_OF` returns the abstract type of module
  `Mo`. The slot's declared kind decides what the engine expects.
- **Module-kind slots.** Type constructors can declare slots that take
  modules. `(SIG_WITH Set ((Elt: Number) (Ord: IntOrd)))` works because
  `Set`'s `Ord` slot is declared `OrderedSig`-kind. Distinct module
  values bound to the same slot give distinct concrete types — the
  mechanism behind witness types in stage 7.

Sharing constraints, modular-implicit signature constraints, and
witness-typed instantiations share this one builtin family. The
implicit *marker* itself (which parameter is implicit) is orthogonal —
see [Modular implicits](#modular-implicits).

## Modular implicits

A function can declare an **implicit module parameter**:

```
LET sort = (FN (SORT xs: List<Mo.Type> {Mo: OrderedSig}) -> List<Mo.Type> = (...))
```

At a call site `(SORT [3, 1, 2])`, the compiler infers `Mo.Type = Number`,
searches in scope for a module satisfying
`(SIG_WITH OrderedSig ((Type: Number)))`, and inserts it. Searching is **lexical**: the candidate set is the implicit
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
uses the `SIG_WITH` builtin described in
[Type expressions and constraints](#type-expressions-and-constraints).
Sugar (block-scoped binding, module priority, selective imports) lands
later,
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
`MakeSet(Er)`'s `gen` builds set samples by drawing from `Er.gen` — gives
the composed module its generator mechanically. Composition is a
module-language property; the engine just calls `Mo.gen` like any other
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

## Inference and search as scheduler work

Type inference and implicit search interleave with dispatch and execution
in the same scheduler. Inference of an expression may need an implicit
search to resolve a parameter; the search may refine type slots that
other inferences are waiting on; sub-expressions are dispatched and
executed by the same engine. There is no separate type-checking pass.
See [execution-model.md § Pegged and free execution](execution-model.md#pegged-and-free-execution) —
build-time and run-time are the same engine, differing only in which nodes
are pegged.

**Inference and search reduce to the existing `Dispatch` and `Bind`
machinery.** Type expressions are evaluated by the same engine values
are. There is no `Infer` node kind; there is no `ImplicitSearch` node
kind; there is no `KType::TypeVar`.

The mechanism:

- **Type-returning builtins are ordinary builtins.** `LIST_OF`,
  `DICT_OF`, `FUNCTION_OF`, `MODULE_TYPE_OF` and the like dispatch and
  execute on the value path; their result is the elaborated type carried
  in `KObject::KTypeValue(KType)`. A `LET MyList = (LIST_OF Number)`
  binding finalizes once and makes `MyList` available as a type name in
  subsequent FN signatures with no per-lookup re-elaboration.
- **Type expressions in source position re-elaborate to a synthesized
  call.** A parameter or return type written as `(LIST_OF Number)` (or
  `List<Number>`) is dispatched directly as a sub-expression whose value
  is a `KType`. Bare type identifiers in FN signatures park on the
  binding's scheduler placeholder via the same `notify_list` /
  `pending_deps` machinery value-name forward references use; recursive
  type definitions short-circuit self-references through the elaborator's
  threaded-set recognition rather than parking on their own placeholder
  ([type-system.md § Type elaboration](type-system.md#type-elaboration)).
- **Refinement rides on `Bind`.** A `Bind` waiting for its sub-Dispatches
  to complete is the existing wake-up mechanism; a type expression that
  tightens later (e.g. as functor application reaches the body) wakes
  its dependents through the same path.
- **Stage 5 implicit search is a single new builtin `SEARCH_IMPLICIT`,
  not a new node kind.** Implicit resolution becomes a Dispatch against
  that builtin with the candidate set assembled from lexical scope; the
  result is a module value, threaded into the call site like any other
  argument.

Rejected: a parallel `Infer` / `ImplicitSearch` node-kind track, with
its own substitution table and `KType::TypeVar`. It would duplicate
scheduling, dependency tracking, cycle detection, and error
propagation that `Dispatch` and `Bind` already provide, and it would
fork the module language away from the value language at exactly the
point — inference — where the metacircular reuse is most valuable.

Properties this preserves:

- **Cycle detection is uniform.** A cycle in implicit resolution and a
  runtime infinite loop are the same kind of bug to the scheduler.
- **Topological ordering falls out.** Dependency-driven wake-up is the
  scheduler's job; type tasks ride the same edges value tasks do.
- **Failure isolation.** Inference and search failures propagate to
  dependents through the existing error-propagation rules; independent
  subtrees still finish, so the user sees multiple errors per build.

This is the **weak metacircular** form: the same scheduler engine that
runs Koan value-language work runs the type checker. The strong form
(compile-time tasks written in Koan and executed by the scheduler) is not
a goal; the architecture leaves the door open without paying its
bootstrapping cost.

The cost-side concession: the design ships first-time-ready refinement
(a type tightens before its dependents fire), not tighten-after-the-fact
(a type tightens after its dependents have already run). If a future
stage — most plausibly stage 5 implicit search — needs the latter, that
motivates a future scheduler primitive; it is not a stage-2 blocker.
See [Open work](#open-work).

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

Implementation stages remain, each producing a usable end state. Stages 1
and 2 — the module language itself (`MODULE`, `SIG`, `:|`, `:!`,
per-module type identity, first-class module values flowing through `LET`
and ATTR) and the module-language substrate through the scheduler
(scheduler-driven type elaborator, `SIG_WITH` sharing constraints,
higher-kinded type-constructor slots) — shipped and are described in the
body above; the Miri audit slate signs them off under tree borrows (see
[memory-model.md § Verification](memory-model.md#verification)).
Signature-bound dispatch (modules-as-values typed against a specific
signature) folds into stage 5.

- [Functor parameters — Type-class names and templated return types](../roadmap/module-system-functor-params.md)
  — two surfaces described in the body (Type-class FN parameters for
  module values, return-type expressions referencing per-call parameters)
  are not yet wired through the runtime.
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
- [Standard library](../roadmap/standard-library.md) — collections built
  as functor FNs over their element/key types. Parks the **applicative
  functor semantics** open question: today's `FN`-based functor surface
  is generative-only, so independent call sites resolving (via stage 5
  implicit search) to the same module still mint distinct output types
  and can't interoperate. Landing form: a separate `FUNCTOR` binder
  reusing FN mechanics, distinguished at the surface so the
  generative/applicative choice is visible at the declaration.
