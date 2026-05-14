# Type identity stage 4 — `NEWTYPE` keyword and `KObject::Wrapped` carrier

Stage 4 of the four-stage type-identity arc. Adds the `NEWTYPE` keyword and
the `KObject::Wrapped` value carrier so a Type-class name can have fresh
nominal identity over a transparent representation. Substrate for
stage-4 axioms ([per-newtype attachment](module-system-4-axioms-and-generators.md))
and stage-5 modular implicits ([per-newtype branches over the same
representation](module-system-5-modular-implicits.md)).

**Problem.** Today there is no language-level mechanism for declaring a
fresh nominal type identity over a transparent representation. `LET Ty =
Number` is a transparent alias — `Ty` and `Number` are observably equal
at dispatch. `STRUCT Ty = (n: Number)` mints a new identity but forces
the declaration to commit to a fields-based shape; `Ty` is not
interchangeable with `Number` at any boundary. Two distinct concepts that share `Number` as
their representation (`UserId`, `PostId`; `Distance`, `Duration`) cannot be
expressed without wrapping each in a single-field STRUCT — heavy syntax for
the same nominal-distinctness payoff.

The same gap blocks downstream features that key on identity:

- [Stage-4 axioms](module-system-4-axioms-and-generators.md) attach
  predicates to a type identity. Without fresh nominal identity over a
  representation, an `AXIOM #(d >= 0)` over `Distance` would have to
  attach to a single-field STRUCT wrapping `Number`.
- [Stage-5 modular implicits](module-system-5-modular-implicits.md) route
  implicit search by type identity. Two `Ordered` implementations over
  `Number` (natural vs reverse) need distinct keys; without fresh
  identity, both compete for `Ordered<Number>`.

Construction is the load-bearing piece: koan dispatches at runtime, so a
value with NEWTYPE identity has to *carry* the NEWTYPE tag in its runtime
carrier. Haskell-style runtime-erased newtypes are incompatible with
koan's runtime dispatch.

**Impact.**

- *Fresh nominal identity over a transparent representation.* `NEWTYPE
  Distance = Number` declares `Distance` as a type distinct from `Number`,
  with `Number` as its representation. `Distance(3.0)` constructs a
  `Distance`-typed value; `f(d)` where `f` takes a `Number` rejects.
- *Per-newtype axiom attachment.* Stage-4 axioms attach to
  `KType::UserType { kind: Newtype, .. }`; distinct-but-same-repr types
  each carry their own predicates.
- *Per-newtype implicit search.* Stage-5 modular implicits key on the
  newtype's identity, not its representation; two `Ordered`
  implementations over `Number` attach to two distinct NEWTYPEs.
- *Field access falls through.* A `KObject::Wrapped` over a
  `KObject::Struct` exposes the inner struct's fields, so wrapping a
  struct in a NEWTYPE doesn't force every field accessor to redo.

**Directions.**

- *`NEWTYPE` keyword — decided.* Lexically classifies as `Keyword`
  (two-or-more-uppercase-letters rule, per
  [type-system.md § Token classes](../design/type-system.md#token-classes--the-parser-level-foundation)).
  Surface: `NEWTYPE Ty = <type-expr>` where `<type-expr>` is the
  representation type.

- *Declaration mechanism — decided.* The NEWTYPE builtin signature is
  `NEWTYPE <name: TypeExprRef> = <repr: TypeExprRef>`. Body mints
  `KType::UserType { kind: UserTypeKind::Newtype { repr: Box<KType> },
  scope_id, name }` and writes only `types["Ty"]` (no `data` write — the
  declaration site has no payload value to bind).

- *`UserTypeKind::Newtype` payload — decided.* `repr: Box<KType>` lives
  inside the `Newtype` variant (variant-internal), not as a sibling field
  on `KType::UserType`. Identity comparison reads `(kind, scope_id, name)`;
  the variant-internal `repr` is not part of identity. A manual
  `PartialEq` impl on `KType::UserType` excludes `repr` from the
  comparison (or an explicit `#[derive]`-friendly shape preserves the
  rule).

- *`KObject::Wrapped` carrier — decided.* `KObject::Wrapped { inner: &'a
  KObject, type_id: &'a KType }`. `inner` is the underlying
  representation value (arena-allocated). `type_id` is the
  `&'a KType::UserType { kind: Newtype, .. }` minted at NEWTYPE
  declaration time. `Wrapped.ktype()` reports `*type_id` (clone).

- *NEWTYPE construction — decided.* `Distance(3.0)` dispatches to a
  NEWTYPE constructor builtin whose signature is `<verb: TypeExprRef>
  <value: KExpression>`. Verb classifies the call as a NEWTYPE
  construction iff `Scope::resolve_type(verb)` returns a `KType::UserType
  { kind: Newtype, repr, .. }` and `value`'s `ktype()` matches `repr`.
  Body produces a `KObject::Wrapped { inner: arena-alloc value, type_id:
  the resolved &KType }`.

- *Newtype-over-newtype collapse — decided.* `inner` is invariantly a
  non-`Wrapped` value. `NEWTYPE Bar = Foo` declared over `NEWTYPE Foo =
  Number`, then `Bar(some_foo)` — the constructor unwraps the inner
  `Wrapped`'s `inner` and rewraps it with `Bar`'s `type_id`, collapsing
  intermediate newtype layers at construction time. Avoids unbounded
  nesting; `Wrapped.inner` is always the bottom-level value.

- *Field access fall-through — decided.* ATTR over a `KObject::Wrapped`
  whose `inner` is `KObject::Struct` reads the inner struct's fields. The
  ATTR builtin's TypeExprRef-lhs path follows the `Wrapped → inner`
  edge automatically for struct / tagged-union inners.

- *Dispatch routing — decided.* `KType::UserType { kind: Newtype, scope_id,
  name }` is just another `UserType` for `matches_value` and
  `is_more_specific_than`; the `kind: Newtype` slot ranks alongside
  `kind: Struct` / `Tagged` / `Module`. `KType::AnyUserType { kind:
  Newtype }` is a wildcard form, parallel to `AnyUserType { kind: Struct }`,
  in case a builtin signature wants "any newtype" matching.

- *Surface name for "any newtype" — open.* Whether `Newtype` is a writable
  wildcard surface name (parallel to `Struct` / `Tagged` / `Module` from
  the shipped stage 3 carrier, see
  [design/type-system.md § Open work](../design/type-system.md#open-work))
  is deferred until a builtin signature surfaces the need.

## Dependencies

**Requires:** none — `KType::UserType`, the `UserTypeKind` enum, the
`try_register_nominal` dual-write path, and the `bindings.types` map have
all shipped (see
[design/type-system.md § Open work](../design/type-system.md#open-work)).
Stage 4 extends `UserTypeKind` with a `Newtype` variant on the existing
substrate.

**Unblocks:**

- [Stage 4 — Property testing and axioms](module-system-4-axioms-and-generators.md)
  — per-newtype axiom attachment (`AXIOM #(d >= 0)` on `Distance`).
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  per-newtype implicit search routing (two `Ordered` implementations over
  `Number` attach to distinct NEWTYPEs rather than competing on the same
  key).
