# Type-slot resolution untangle

`of_kind(ProperType)` becomes purely a kind expectation; the "body owns
resolution" duty it silently carries moves to an explicit thunk carrier, and
window-aware resolution moves into the shared elaboration lane.

**Problem.** `KType::PROPER_TYPE` carries two unrelated duties on one constant.
As a slot type it is a kind expectation — "a type value of kind `*`", ordered by
the `KKind` lattice. But `KFunction::classify_for_pick`
([`pick.rs`](../../src/machine/core/kfunction/pick.rs)) also excludes it from
the bare-name auto-wrap, so a bare `Type`-token operand at such a slot reaches
the body as an unresolved token the body must resolve itself. Nine slot sites
across seven builtins depend on that raw token, and they need it for three
different reasons:

- *Deferral*: `fn_def`'s return and signature slots and `op_def`'s operand and
  return slots may reference an FN parameter unbound in the defining scope, so
  elaboration must wait for the dispatch boundary
  ([`return_type.rs`](../../src/builtins/fn_def/return_type.rs) classifies the
  raw carrier into done / pending / deferred).
- *Window context*: `attr`'s type-lhs projection folds a sibling reference
  under a still-open seal to a relative `Sibling` handle instead of parking on
  the seal awaiting it; `newtype_def` threads its own binder name; `val_decl`
  resolves inside a SIG body window.
- *Diagnostic ownership*: `match_case` / `try_with` / `type_ops` slots resolve
  synchronously against the call-site chain purely to render a pointed error
  ("MATCH return type `Bogus` is not a known type"). Scheduling gains nothing:
  a non-binder form's `Type`-token operands already park at dispatch on
  backward in-flight producers
  ([`resolve_dispatch.rs`](../../src/machine/execute/decide/resolve_dispatch.rs)),
  so by body time only genuinely unbound names remain.

The raw-capture duty is invisible from the slot's spelling. A builtin that
wants only the kind expectation and writes the obvious `of_kind(ProperType)`
silently opts into raw capture and receives a token it never asked to resolve —
which is how the `FN` type constructor's parameter-list slot rejected a
`LET`-bound record alias (`:(FN Params -> Bool)` failing with `expected
ProperType, got Params`), shipped around by widening that slot to
`of_kind(AnyType)`
([type-language-via-dispatch.md](../../design/typing/type-language-via-dispatch.md)).
The same hole is open today in the anonymous FN definition form: `fn_def`'s
`signature` slot is `of_kind(ProperType)`, so `LET Params = :{x :Number}` then
`FN Params -> Number = (x)` fails with `anonymous FN signature slot must be a
record schema :{…}` while a `:{…}` literal works.

**Acceptance criteria.**

- `classify_for_pick`'s auto-wrap exclusion names only literal-name slot types
  (`IDENTIFIER`, `NAME_TOKEN`, `TYPE_NAME_TOKEN`): a bare `Type` token at an
  `of_kind(ProperType)` slot of any overload arrives in the body as a resolved
  type carrier, and the `WrapIndices` doc comment states the literal-name rule
  it actually implements.
- The deferral slots (`fn_def` return and signature, `op_def` operand and
  return) spell raw capture explicitly as unions of part-kind-exact carrier
  members, and the body-side three-way probe (`extract_type_slot_raw`'s
  resolved / unresolved-name / raw-expression arms) reads one normalized
  thunk carrier instead of probing three accessors.
- The window sites (`attr` type-lhs projection, `newtype_def` repr, `val_decl`
  ty) receive lane-resolved carriers, and the behaviors their body protocols
  pinned still hold: sibling-under-seal projection folds to a relative
  `Sibling` handle, a recursive `NEWTYPE` threads its own name, and the
  functor / cross-sig `attr` and `ascribe` suites stay green.
- The `match_case` / `try_with` / `type_ops` type slots splice, and their
  diagnostics still name the form and role with the offending name (`MATCH
  return type `Bogus` is not a known type`, or equal pointedness).
- `LET Params = :{x :Number}` then `FN Params -> Number = (x)` defines a
  function that runs — the anonymous-definition alias hole closes at parity
  with the shipped `:(FN Params -> Bool)` constructor surface.
- No scheduling change: forward type references stay position errors on every
  surface, and backward in-flight references still park at dispatch time
  exactly as today.

**Directions.**

- *Carrier mechanism — decided.* Union admission plus an internal thunk seam.
  The shipped
  [union carrier slots](../../design/typing/ktype/slots-and-signatures.md#union-carrier-slots)
  carry admission: each
  deferral slot is one overload with a `union_of(TYPE_NAME_TOKEN,
  SIGILED_TYPE_EXPR, RECORD_TYPE, IDENTIFIER)` slot — every member
  part-kind-exact and captured raw by its own explicit semantics, no sentinel.
  `TYPE_NAME_TOKEN` is reused as the raw-token member: its mechanical meaning
  is already "raw `Type` token, the body owns it", and whether a body *binds*
  the token or *resolves* it is the form's role — registered in
  `BINDER_SPECS` ([`binder.rs`](../../src/machine/model/binder.rs)), which is
  what the dispatch park exemption keys on, never the slot constant. The
  thunk is the body-side normalization of whatever member matched — resolved
  type | unresolved name | raw type expression, forced with the body's own
  scope, window, and chain, or carried to the dispatch boundary for per-call
  elaboration. It is internal to builtin bodies, not a koan-visible value.
- *Window-aware lane mechanics — open.* How `attr`'s sibling-under-seal fold
  and `newtype_def`'s self-threading move lane-side. Precedents:
  `rewrite_threaded_self_refs` pre-splices `Sibling` handles before a
  sub-dispatch, and the `under_type_sigil` stamp carries a per-node fact
  through resplices
  ([type-language-via-dispatch.md § Classifier](../../design/typing/type-language-via-dispatch.md#classifier)).
- *Diagnostic plumbing — open.* How a spliced slot's unbound-name error keeps
  the form-and-role noun: a slot-role label flowing into the lane's error, or
  a single raiser shared with
  [Uniform forward type references](uniform-forward-type-references.md)'
  one-diagnostic criterion.
- *Re-narrowing the FN constructor slot — open.* With the auto-wrap decoupled,
  the FN type constructor's parameter-list slot could return from
  `of_kind(AnyType)` to `of_kind(ProperType)` so a signature value is refused
  at the kind level; record-ness stays a body check either way. Cost: the
  pointed `must be a record type` error currently covers that case.

## Dependencies

This item interacts with
[Uniform forward type references](uniform-forward-type-references.md) (a
shared single diagnostic raiser is an option, not an edge).

**Requires:** none — the union carrier slots its deferral slots spell raw
capture with are shipped.

**Unblocks:** none.
