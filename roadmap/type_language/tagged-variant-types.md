# Tagged-union variants as dispatchable types

Promote each `UNION` variant from a value-side string label to its own nominal
`KType`, so tagged-union elimination collapses into ordinary type-dispatch.

**Problem.** koan's nominal tagged unions carry their variant labels as
value-side string identifiers, not types. `UNION Maybe = (some :Int, none :Null)`
parses each tag through `FieldNameKind::Identifier` — strict lowercase user
identifiers (`src/parse/triple_list.rs`) — stores the schema as
`Rc<HashMap<String, KType>>` keyed by tag-string
(`NominalKind::Tagged`, `src/machine/model/types/ktype.rs`), and a value carries
its tag as a plain `String` (`KObject::Tagged`, `src/machine/model/values/kobject.rs`).
Construction extracts the first call argument as a bare `Identifier` and looks it
up by string key (`src/machine/execute/dispatch/constructors/tagged_union.rs`);
elimination is a bespoke `MATCH` form that re-extracts the tag string and matches
arms by name (`src/builtins/match_case.rs`). A variant is therefore invisible to
the type language: it has no `KType` identity, can't fill a typed slot, can't be
dispatched on, and `MATCH` re-implements discrimination as string comparison
instead of reusing the type-dispatch machinery that already eliminates every
other typed value. A tag classifies as `BareIdentifier`, never `BareTypeLeaf`
(`classify_dispatch_shape`, `src/machine/model/ast.rs`).

**Acceptance criteria.**

- A declared variant mints a `KType` refinement of its union, so a slot can be
  typed to a single variant and a function accepting only `some` rejects `none`
  at bind time.
- Tagged-union elimination runs through ordinary type-dispatch — the same
  mechanism that eliminates [anonymous structural
  unions](anonymous-unions.md) by runtime type — with `MATCH` lowering to
  type-dispatch rather than a parallel string-matching form.
- Discrimination keys on variant-type identity, so `UNION R = (ok :Int,
  error :Int)` keeps two distinct arms.
- A variant is usable inside `:(...)`, as an agreed return type, and as a
  dispatch key, classifying as a type leaf rather than through
  `BareIdentifier`.
- Each variant is a `Newtype` over its payload and the union is their
  anonymous-union join, so `NominalKind::Tagged` dissolves into `Newtype`;
  combined with the product-side [struct → record-repr `NEWTYPE`
  collapse](struct-newtype-collapse.md), `NominalKind` is reduced toward
  `{Newtype, TypeConstructor}`.

**Directions.**

- *Variant identity as its own `KType` — decided.* Each declared variant mints a
  nominal `KType` refinement of its union, distinct from its payload type; the
  union type is the join of its variant types (each variant a subtype of the
  union), mirroring the member/union subtyping of
  [anonymous structural unions](anonymous-unions.md). Discrimination keys on
  variant identity, not payload type, so same-payload variants stay distinct.
- *Tag namespace — open.* Where variant types live. Options: (a) global type
  names (`some`) — simplest, but collides across unions and loses today's free
  per-union namespacing; (b) union-scoped path (`Maybe.some`) — collision-free,
  but needs a member-path surface koan lacks; (c) structurally keyed by
  `(union-identity, tag)`, reachable only through the union — collision-free with
  no new global names, mirroring opaque-member identity
  (`AbstractSource` / `Wrapped`). Recommended: (c), with (b) layered on if a path
  surface lands.
- *Lexical reclassification — decided.* Tags parse as type-leaf tokens
  (`BareTypeLeaf`) rather than `FieldNameKind::Identifier`
  (`src/parse/triple_list.rs`), so a variant is type-classified everywhere
  `classify_dispatch_shape` runs. Whether type-leaf lexing forces a
  capitalization convention (`Some` vs `some`) rides the same tokenizer change.
- *MATCH vs dispatch — open.* Whether `MATCH` becomes pure sugar lowering to
  type-dispatch arms, or stays a distinct form that reuses the variant-type
  machinery internally. The tag-free "match by type" arm shape is the same sugar
  [anonymous-unions](anonymous-unions.md) defers. Recommended: keep `MATCH` as
  surface sugar that lowers to type-dispatch.
- *Construction surface — open.* Whether construction stays union-name-led
  (`(Maybe (some 42))`) or becomes variant-led (`(some 42)`) with the union
  inferred from the variant type. Recommended: defer until the tag namespace is
  settled — variant-led construction presumes a reachable variant name.

## Dependencies

Cross-link (not a dependency edge): [anonymous structural
unions](anonymous-unions.md) shares the type-dispatch elimination model — that item
handles *untagged* unions, this one supplies the variant `KType` so *tagged* unions
eliminate the same way — but neither blocks the other.

**Requires:**

- [Type-only nominal identities](../../design/typing/user-types.md) — the shipped
  `NominalKind::Tagged` schema and type-side-only nominal install this work
  re-shapes into per-variant `KType` identities.
- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md)
  — variant types ride the same `:(...)` / dispatch substrate that eliminates
  every other typed value.
- [Branch-arm return contract](../../design/execution-model.md#arms-as-own-blocks)
  — the `MATCH` arm machinery this work lowers into type-dispatch.
- [Record-repr NEWTYPE recursion](struct-newtype-collapse.md) — establishes the binder-name
  back-edge threading a recursive tagged union reuses to thread a `SetLocal` through its
  union name. (The product-side `Newtype` primitive each variant builds on already shipped.)

**Unblocks:** none tracked yet.
