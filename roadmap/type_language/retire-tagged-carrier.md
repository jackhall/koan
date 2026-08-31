# Retire the Tagged carrier

One nominal wrap carrier — `KObject::Wrapped` — for newtypes, union variants,
`Result`, and lowered errors: identity discriminates, and no tag symbol rides
the value.

**Problem.**
[`KObject::Tagged`](../../src/machine/model/values/kobject.rs)` { tag, value,
identity }` and `KObject::Wrapped { inner, type_id }` are near-twins — one
payload substrate plus a type identity — and the `tag` is derivable from the
identity's member node everywhere except `Result`:
[`result.rs`](../../src/builtins/result.rs) registers a **single**
`TypeConstructor` member whose schema is `{Ok: Any, Error: Any}`, so both
variants share one identity handle and the tag symbol is the only
discriminant. Downstream of that choice:
[`kerror.rs`](../../src/machine/core/kerror.rs)`::to_tagged` mints synthetic
singleton identities solely because `TRY`'s walker reads `tag` and `value`
without dispatch, and
[`branch_walk.rs`](../../src/builtins/branch_walk.rs)`::find_branch_body_by_tag`
is a second branch-selection regime beside `MATCH`'s member walk.

**Acceptance criteria.**

- `KObject::Tagged` no longer exists; `Result` values and lowered `KError`
  values ride `Wrapped`, with a member handle (or `ConstructorApply` over one)
  as `type_id`.
- `Result` is registered as a sealed two-member group — `Ok` and `Error`, each
  over `Any` — bound to the members' union; `:(Result {Ok = …, Error = …})`
  still resolves, and a slot so typed still runtime-checks the inhabited
  member's payload against the same-named type argument.
- The catchable `KErrorKind` surface set is registered as members of a prelude
  error union; `KError` lowering constructs `Wrapped` values carrying those
  member handles, and no synthetic singleton identities remain.
- A caught error renders each name once: `PRINT (CATCH (mystery))` reads
  `Error(UnboundName({frames = [], name = mystery}))`. The `Tagged` tag and the
  synthetic-singleton `Wrapped` identity both spell the variant name today, so
  every caught error renders it twice — `Error(UnboundName(UnboundName({…})))`
  — for any kind, and a tutorial example showing a failed `CATCH` becomes
  possible again.
- `TRY` selects arms through the same member walk as `MATCH … OVER`, with `Ok`
  plus the error members as its fixed member set; the `_` wildcard's reach —
  including the dispatcher-internal kinds — is preserved.
- `MATCH r OVER Result WITH (…)` eliminates a `Result` value through the
  member walk, and the `MATCH`-over-`Result` behavioural tests disabled during
  the surface rework are re-enabled on that spelling.
- Equality, lift, `matches_value`, and the substrate paths carry variant
  values through the `Wrapped` arm, and the carrier-level docs
  ([value-substrates.md](../../design/value-substrates.md),
  [label-interning.md](../../design/label-interning.md),
  [value-equality.md](../../design/execution/value-equality.md),
  [parameterization-and-variance.md](../../design/typing/ktype/parameterization-and-variance.md),
  [calls-and-values.md](../../design/execution/calls-and-values.md)) describe
  the single-carrier model.

**Directions.**

- *`Result` remodel — decided.* The union-as-newtype machinery a user `UNION`
  uses — two sealed members bound to their union — replaces the single-member
  `TypeConstructor` whose tag discriminated.
- *Error union — decided.* A closed member set minted from the `KErrorKind`
  surface names at prelude registration.
- *Parameterized identity — open.* Where the `Ok` / `Error` type arguments
  live once the members split: a `ConstructorApply` over the inhabited member
  (recommended — `Wrapped` already carries per-member applications for
  `NEWTYPE (T AS W)`), or a union-level application.
- *Wildcard home — open.* How `_` and the dispatcher-internal kinds ride the
  unified member walk — a default-arm rule the walk understands, shared with
  or distinct from `MATCH … OVER`'s open default-arm question.

## Dependencies

**Requires:** none — the member walk and projection-based construction it
migrates `Result` and `TRY` onto have shipped.

**Unblocks:** none — a leaf consolidation.
