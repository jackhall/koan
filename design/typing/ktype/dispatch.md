# Dispatch and slot specificity

How slot specificity ranks overloads, and the per-overload visibility filter.
Part of the [`KType` reference](README.md).

## Dispatch and slot-specificity

When multiple registered functions match an incoming expression, dispatch picks
by slot-specificity: typed slots outrank untyped ones; literal-typed slots
outrank `Any`. See [expressions-and-parsing.md](../../expressions-and-parsing.md) for
how the parser splits an expression into the `Keyword`/slot positions that
specificity scores against.

**Container slots admit on the carried element type, not on shape alone.** An
*unevaluated* container literal (`ListLiteral` / `DictLiteral`) is admitted
shape-only — its element types aren't known until it evaluates. An *evaluated*
container (`Future(List/Dict)`) is admitted only when its memoized carried element
type *satisfies* the slot (`KType::satisfied_by`: exact match or covariant
refinement) — a pure type-level comparison against the value's `ktype()`, with no
element walk. A `List<Number>` value fills `:(LIST OF Any)`; a `List<Any>` value (the
join an empty or heterogeneous literal memoizes) fills `:(LIST OF Any)` but not
`:(LIST OF Number)`. A container whose carried type doesn't satisfy a slot is a
*non-match*: dispatch falls through to outer scopes and, finding nothing, surfaces
`DispatchFailed` rather than committing to a slot that would fail at the bind
boundary.

This makes element-only-differing overloads (`:(LIST OF Number)` vs `:(LIST OF Str)`)
dispatchable across the forms a container argument takes. Admission is
strict-only, driven by a per-dispatch-poll `bare_outcomes` cache —
[`signature_admits_strict`](../../../src/machine/execute/decide/resolve_dispatch.rs)
reads each bare-name slot's cached
[`Resolution`](../../../src/machine/execute/decide/resolve.rs) once and
admits accordingly. The forms:

- **Evaluated argument** (`DESCRIBE (xs)`, a call result) — the scheduler has
  already spliced the resolved sub-result in as a
  [`WorkingPart::Spliced`](../../../src/machine/model/ast/working.rs) cell, so
  admission opens the cell and runs
  [`KType::accepts_cell`](../../../src/machine/model/types/ktype_predicates.rs)
  for the carried-type check — no part shape is consulted.
- **Bare variable** (`DESCRIBE xs`) — the cache entry is
  `Resolution::Resolved(Delivered)`. Admission opens the delivered envelope and
  tests
  [`KType::accepts_carried`](../../../src/machine/model/types/ktype_predicates.rs)
  against the carried value (an object or a `Type` arm — no clone). A bare name whose value has the
  wrong carrier type strict-rejects the overload; the call surfaces as `DispatchFailed`
  rather than a bind-time `TypeMismatch`. Binder (`Identifier` / `OfKind(Proper)`)
  slots skip the cache and admit shape-only — the slot owns the name, so
  admission can't depend on whether `x` happens to be bound or parked. A
  `:KExpression` slot does consult the cache, because code is an ordinary
  value: a name bound to a quote carries a `KExpression` and admits.
- **Literal** (`DESCRIBE [1 2 3]`) — the cache entry is `None` (literals
  aren't bare names) and admission runs `arg.matches(part)` shape-only.
  A literal is staged as its own sub-`Dispatch` before resolution runs, so
  what a typed slot actually admits is the resulting `Future`, element-aware
  and specific enough to break what the bare shape would have tied. A tie
  that survives evaluation (e.g. an empty list against two concrete-element
  overloads, both admitted vacuously) surfaces as `AmbiguousDispatch`.

A `Placeholder` (forward reference) cache outcome pre-empts admission
entirely: resolution parks on the binder's producer before any candidate
is consulted and re-dispatches once it binds — the rebuilt cache carries
`Resolved(obj)` and strict admission picks against the landed carrier.
This keeps dispatch order-independent within the visibility window —
`DESCRIBE xs` resolves to the same overload whether or not `LET xs = …`
had landed at first dispatch, provided the binding is lexically visible
to the reference (see
[Overload bucket visibility filter](#overload-bucket-visibility-filter)).
The pre-scan's exemptions — a binder form's declared-name position and
its `Type`-token operands, both read off the expression's cached
spec-table facts — are covered in
[elaboration.md § Strict admission rules](../elaboration.md#strict-admission-rules).

An `Unbound` cache outcome rejects at every typed value slot (a name that
resolves to nothing satisfies none). If no bucket admits anywhere, the
resolver's post-walk fallback surfaces
`DispatchOutcome::UnboundName(name)` from the relaxed pass's dead lean —
the precise error matching what the single-overload path reports for an
unresolved bare name, not a generic dispatch miss.

Specificity ranks `is_more_specific_than` so that concrete carrier types
beat the unconstrained-name slot types (`Identifier` / `OfKind(Proper)`). A
call like `ATTR p z` where `p` resolves to a record value admits both a
concrete-typed `ATTR` overload and an `ATTR <s:Identifier>` fallback;
the concrete overload wins by specificity without tying.

`Str` is the one exception to that rule: **`Identifier` out-specifies `Str`**,
and `Str` does not out-specify `Identifier`. The two slots read the same bare
token at different depths — an `Identifier` slot claims the token *itself*, a
`Str` slot only the value the token resolves to — so when one bucket offers both
readings, the token reading wins. A field token stays bare wherever an
`Identifier` slot admits it, and a local string binding that happens to share the
spelling cannot steal it. The pair has no other consumer: `Identifier` is not
user-spellable, and no other builtin bucket puts the two slot types in one
position, so the ranking alone decides and dispatch admission needs no carve-out.

`ATTR`'s `field` position is where that matters. The spelled forms — the `.`
sugar (`s.x`) and the written-out `ATTR s x` alike — bind the field bare through
an `Identifier` slot. A field that arrives as a runtime string instead falls to a
**pair** of dynamic overloads, split by the lhs exactly as the bare-token
overloads are: `ATTR <s :Any> <field :Str>` reads a member off a runtime value,
and `ATTR <s :EmptySignature> <field :Str>` answers out of a module's own
bindings, `EmptySignature` (which every module's self-sig satisfies)
out-specifying `Any`. So `s."x"` and `ATTR s (name_var)` reach the same member
`s.x` does — the dynamic read classifies and interns the text at the read, which
is the value channel's one derived-symbol door
([label-interning.md](../../label-interning.md)) — while `ATTR p x` with `x`
bound to a string still reads the member named `x`.

### Overload bucket visibility filter

Function-bucket lookup pre-filters by per-overload visibility before the strict
admit predicate runs — the [lookup → admit protocol](../lookup-protocol.md)'s
Layer 2 (`Bindings::lookup_function_stored`) applied per-overload rather than per
name. Each `functions` entry carries a per-overload
[`BindingIndex { idx }`](../../../src/machine/core/bindings.rs) — the lexical
statement index at which the overload was registered. The visibility predicate
is `idx < cutoff`, one rule across the value and type languages. A consumer
between two same-bucket overloads sees only the earlier; the later-sibling
overload is hidden, and dispatch falls through to outer scopes unaffected by the
not-yet-visible registration.
[`OverloadBucket::pick_strict`](../../../src/machine/execute/decide/resolve_dispatch.rs)
receives the pre-filtered survivor list (the `FunctionLookup`'s `overloads`)
and runs only the admit predicate over it. The same lookup also surfaces the
earliest-index visible *claim* on that same bucket key in `FunctionLookup`'s
`pending` field; a visible claim parks that scope for a park-and-replay on
wake, since it would shadow once finalized.

The result: an FN reference resolves under the same lexical-position rule as a
value-LET reference, and a bare forward reference inside a sibling expression
surfaces `UnboundName` directly — visibility is lexical, and the parking edges
are reserved for visible-but-not-ready producers.

