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
[`signature_admits_strict`](../../../src/machine/execute/dispatch/resolve_dispatch.rs)
reads each bare-name slot's cached
[`NameOutcome`](../../../src/machine/execute/dispatch/resolve_dispatch.rs) once and
admits accordingly. The forms:

- **Evaluated argument** (`DESCRIBE (xs)`, a call result) — already a typed
  `Future`; admission runs `arg.matches(part)` and `accepts_part` for the
  carried-type check.
- **Bare variable** (`DESCRIBE xs`) — the cache entry is
  `NameOutcome::Resolved(Carried)`. Admission tests
  [`KType::accepts_part`](../../../src/machine/model/types/ktype_predicates.rs)
  against `ExpressionPart::Future(Carried)` (the `Future` arm holds a `Carried`
  reference — an object or a `Type` arm — no clone). A bare name whose value has the
  wrong carrier type strict-rejects the overload; the call surfaces as `DispatchFailed`
  rather than a bind-time `TypeMismatch`. Binder (`Identifier` / `OfKind(Proper)`) and
  lazy (`KExpression`) slots skip the cache and admit shape-only — the slot
  owns the name, so admission can't depend on whether `x` happens to be
  bound or parked.
- **Literal** (`DESCRIBE [1 2 3]`) — the cache entry is `None` (literals
  aren't bare names) and admission runs `arg.matches(part)` shape-only.
  Both element-typed overloads admit and the strict pass *ties*. The
  dispatch driver treats a strict tie whose argument carries unevaluated
  eager parts as `Deferred` rather than `AmbiguousDispatch`; the literal
  evaluates and the re-dispatch on the resulting typed `Future` is
  element-aware. A tie that survives evaluation (e.g. an empty list
  against two concrete-element overloads, both admitted vacuously)
  carries no eager parts on the second pass and surfaces as
  `AmbiguousDispatch`.

`Placeholder` (forward reference) and `Unbound` cache outcomes admit via
shape-only `arg.matches(part)` rather than carrier-type check. The
post-pick splice/park walk is the only place that produces precise per-slot
`ParkOnProducers` / `UnboundName` diagnostics, so admission must not
reject them. If no bucket admits anywhere, the resolver's post-walk
fallback reads the cache by fixed precedence — placeholders > eager >
unbound > pending overload > Unmatched — and surfaces the right
`ResolveOutcome`:

- A `Placeholder` name *will* bind, so the fallback surfaces
  `ResolveOutcome::ParkOnProducers(producers)`. Dispatch parks on the
  binder's producer and re-dispatches once it binds; the rebuilt cache
  carries `Resolved(obj)` and strict admission picks. This keeps dispatch
  order-independent within the visibility window — `DESCRIBE xs` resolves
  to the same overload whether or not `LET xs = …` had landed at first
  dispatch, provided the binding is lexically visible to the reference
  (see [Overload bucket visibility filter](#overload-bucket-visibility-filter)).
  Park parking goes through the same edges as the resolved-pick
  replay-park.
- An `Unbound` name names nothing (no visible binding *and* no
  forward-declared placeholder visible at the consumer's chain position),
  so the fallback surfaces `ResolveOutcome::UnboundName(name)` — the
  precise error matching what the single-overload path reports for an
  unresolved bare name, not a generic dispatch miss.

Specificity ranks `is_more_specific_than` so that concrete carrier types
beat the unconstrained-name slot types (`Identifier` / `OfKind(Proper)`). A
call like `ATTR p z` where `p` resolves to a record value admits both a
concrete-typed `ATTR` overload and an `ATTR <s:Identifier>` fallback;
the concrete overload wins by specificity without tying.

### Overload bucket visibility filter

Function-bucket lookup pre-filters by per-overload visibility before the strict
admit predicate runs — the [lookup → admit protocol](../lookup-protocol.md)'s
Layer 2 (`Bindings::lookup_function`) applied per-overload rather than per
name. Each `functions` entry carries a per-overload
[`BindingIndex { idx }`](../../../src/machine/core/bindings.rs) — the lexical
statement index at which the overload was registered. The visibility predicate
is `idx < cutoff`, one rule across the value and type languages. A consumer
between two same-bucket overloads sees only the earlier; the later-sibling
overload is hidden, and dispatch falls through to outer scopes unaffected by the
not-yet-visible registration.
[`OverloadBucket::pick_strict`](../../../src/machine/execute/dispatch/resolve_dispatch.rs)
receives the pre-filtered survivor list (the `FunctionLookup`'s `overloads`)
and runs only the admit predicate over it. The same lookup also surfaces the
earliest-index visible `pending_overloads[key]` producer in `FunctionLookup`'s
`pending` field; a visible pending parks that scope for a park-and-replay on
wake, since it would shadow once finalized.

The result: an FN reference resolves under the same lexical-position rule as a
value-LET reference, and a bare forward reference inside a sibling expression
surfaces `UnboundName` directly — visibility is lexical, and the parking edges
are reserved for visible-but-not-ready producers.

