# FN/FUNCTOR named identity

Load parameter names from the sigil surface into `KType` identity so
function-typed slots can mechanically enforce that callers use the
declared parameter names.

**Problem.**
[`type-language-via-dispatch`](../dispatch_fix/type-language-via-dispatch.md)
introduces the `:(FN (x :Number, y :Str) -> Bool)` and
`:(FUNCTOR (T :SomeSig) -> Module)` sigil surfaces, which declare
parameter names at the type position. Lowering drops the names:
[`KType::KFunction { args, ret }`](../../src/machine/model/types/ktype.rs)
and `KType::KFunctor { params, ret }` store args positionally and
compare by structural equality, so `:(FN (a :Number) -> Bool)` and
`:(FN (b :Number) -> Bool)` are identity-equal. Calls through a
function-typed slot still require named args
([execution-model.md](../../design/execution-model.md): "koan has no
`f 1 2` positional call syntax"), but the function-typed slot itself
has no record of which names the callee expects, so the use-site
constraint can't be checked against the slot's type.

**Impact.**

- Function-typed slot use sites enforce the declared parameter names
  mechanically — passing `g(a = 1)` through a slot typed
  `:(FN (b :Number) -> Bool)` is a structural mismatch caught at the
  slot boundary, not deferred to the dispatch attempt at the callee.
- Functor-typed slots gain the same enforcement against their declared
  parameter names.
- The named sigil surface stops being a documentation hint —
  parameter names round-trip through `KType` identity and back to the
  rendered form.

**Directions.**

- **Storage shape — open.** Two candidates: (a) a `Vec<(String,
  KType)>` parallel to today's `Vec<KType>` in `KFunction.args` /
  `KFunctor.params`; (b) a parallel `param_names: Vec<String>` field
  on each variant. Option (a) keeps the (name, type) pair grouped at
  the storage site; option (b) leaves the existing `args` /
  `params` walks untouched and adds the names alongside.
  *Recommended: (a).* The walks that care about types-only can map
  `.map(|(_, t)| t)`; the lookups that care about (name, type) read
  the pair directly without zipping.
- **Equality semantics — decided: identity by `(name, type)` pairs in
  order.** Two `KFunction` types are equal iff their arg sequences
  agree pairwise on both name and type. Same for `KFunctor`.
- **Rendering — decided: include names.**
  `KType::name()` renders as `:(FN (x :Number, y :Str) -> Bool)` so a
  round-trip through the parser produces the same `KType`.
- **Builtin / FFI carriers — open.** Builtin function carriers
  registered without parameter names (most operators) need a synthetic
  naming convention (`_0`, `_1`, …?) or an opt-out. *Recommended:
  positional placeholders that admit-equal to any name at the slot
  boundary, so existing builtin sites don't have to acquire fake
  names.*
- **Structural-equality migration — open.** Today's tests in
  `src/machine/model/types/ktype.rs` (`assert_eq!(t.name(),
  ":(Function (Number Str) -> Bool)")`) assume positional rendering.
  Every test and every call site that compares function types needs
  re-checking. Scoped by a grep for `KType::KFunction` / `KFunctor` /
  the `(Function …)` / `(Functor …)` rendered forms.

## Dependencies

**Requires:**
- [Type language via dispatch](../dispatch_fix/type-language-via-dispatch.md) —
  ships the named sigil surface (`:(FN (x :Number) -> Bool)` etc.)
  that this item loads into `KType` identity. The lowering drops
  names today; this item picks them up.
