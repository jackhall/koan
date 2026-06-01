# User-defined TypeConstructor keyworded application

Give a user-declared `TypeConstructor` a dispatchable keyworded
application surface so `:(Wrap Number)` (or a renamed form) routes
through the standard type-language dispatch path that
`:(LIST OF Number)` and friends already use.

**Problem.** Only the four builtin parameterized type heads (`LIST`,
`MAP`, `FN`, `FUNCTOR`) ship with keyworded overloads in
[`builtins/type_constructors.rs`](../../src/builtins/type_constructors.rs);
a user-declared
[`TypeConstructor`](../../src/builtins/type_ops/type_constructor.rs)
like `LET Wrap = (TYPE_CONSTRUCTOR T)` has no parallel registration.
A sigil application `:(Wrap Number)` parses as
`SigiledTypeExpr([Type(Wrap), Type(Number)])`, sub-Dispatches the
inner expression, and lands in the dispatcher's
`ConstructorCall` arm — which routes
`StructType` / `TaggedUnionType` / `Newtype` heads through their
construction primitives but has no path for a
`UserType { kind: TypeConstructor { .. } }` head. The user-defined
TypeConstructor cannot be applied through the sigil at all today.

Two test ignores pin the gap:

- `fn_return_type_constructor_apply_root_scope` in
  [`src/builtins/type_ops/type_constructor.rs`](../../src/builtins/type_ops/type_constructor.rs)
- `monad_signature_smoke` in the same file — uses
  `:(FN (x :Number) -> :(Wrap Number))` inside a `SIG` body's
  `VAL pure` declaration.

**Impact.**

- A user-declared `TypeConstructor` is a full citizen of the
  type-language: `LET Box = (TYPE_CONSTRUCTOR T)` declared in one
  scope is callable as `:(Box Number)` (or the chosen surface form)
  anywhere `LIST OF` / `MAP` are.
- The `SIG` body `VAL` slot accepts user-functor return types
  uniformly — `monad_signature_smoke` becomes a round-trip rather
  than a known-broken pin.
- The TypeConstructor's slot-shape (one positional `:Type` arg per
  declared param) projects naturally into a dispatchable signature
  that lives alongside the builtin keyworded overloads, with the
  same `KTypeValue` output shape.

**Directions.**

- **Surface form — open.** Two candidates:
  (a) auto-register a positional `:(Wrap T)` overload at
  `TYPE_CONSTRUCTOR` declaration time, mirroring the legacy
  positional sigil shape;
  (b) require users to declare a keyworded surface
  (e.g. `:(WRAP OF T)` or `:(Wrap (T = Number))` function-value
  style) at declaration time, and route through that.
  *Recommended: (a).* The positional shape matches what users
  already expect from the builtin keyworded overloads they've seen
  for `LIST OF` / `MAP` — and it lines up with the
  `ConstructorCall` classifier arm that handles the rest of
  the leaf-Type-headed multi-part call space. (b) trades discovery
  cost for naming flexibility users likely don't need yet.
- **Auto-registration site — decided per (a):
  `TYPE_CONSTRUCTOR`'s body.** The body that records the
  `UserType { kind: TypeConstructor { param_names }, .. }`
  binding also calls `register_function` for a synthetic
  positional overload keyed on the constructor's name. The
  signature is `[Type(name), :Type, :Type, ...]` with one `:Type`
  slot per declared parameter; the body wraps the resolved args as
  `KType::ConstructorApply { ctor, args }` and returns a
  `KTypeValue` carrier.
- **SCC-recursion handling — deferred.** Self-recursive
  declarations like `LET RoseTree = (TYPE_CONSTRUCTOR T)` followed
  by `STRUCT (RoseTree T) = (kids :(LIST OF (RoseTree T)))` build on
  the shipped bare-leaf self-reference pre-resolution (see
  [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md)),
  but additionally need a threaded constructor *application* head
  (`(RoseTree T)`), not just a bare leaf name, rewritten to a
  recursive carrier — that composes on top of this item's keyworded
  application surface.
- **Per-call generativity — decided: per
  [functors.md § Higher-kinded type slots](../../design/typing/functors.md#higher-kinded-type-slots).**
  Each `:(Wrap Number)` application produces a fresh
  `ConstructorApply` carrier with the per-call generativity rules
  the existing `TYPE_CONSTRUCTOR` builtin already enforces; this
  item adds the surface, not the identity semantics.

## Dependencies

**Requires:**

- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md)
  (shipped) — the substrate that routes every parameterized type
  through the dispatcher and the registered overloads.

**Unblocks:** none directly; closes a user-facing surface gap and
removes two known-broken test pins.
