# Type language via dispatch

Lift parameterized type construction onto the dispatch substrate.

**Problem.** The parser folds sigiled type expressions like
`:(List Number)` and `:(Dict Str Number)` at parse time into a single
`ExpressionPart::Type` with `TypeParams::List(...)` (see
[`type_expr_frame.rs`](../../src/parse/type_expr_frame.rs)). The fold is
hand-keyed to a fixed list of builtin heads (`List`, `Dict`, `Function`,
`Functor`) and uses a positional grammar the value language never gets.
Adding a parameterized type requires a parser-side wiring step instead
of a builtin registration. The dispatcher's
[`TypeCall`](../../src/machine/execute/scheduler/dispatch.rs) classifier
shape (`(List Number)` no-sigil) carries no real user surface — the
only callers are synthetic scheduler tests. User-defined functors can
only be invoked in non-sigil position; the sigil parses the `KFunctor`
*type* shape but never a functor *application*, so a sigiled call like
`:(MyFunctor IntOrd)` has no path through today's grammar.

**Impact.**

- Parameterized type construction is uniformly a builtin registration.
  Adding `MAP`, a future `SET`, or any user-defined parameterized
  carrier reuses the candidate-bucket and binder-admission machinery
  rather than parser-side wiring.
- The sigil becomes a parse-context marker only — `:(...)` flags
  "evaluates to a type" without taking responsibility for the inner
  expression's structure. The parser stays simple; dispatch handles the
  shape decisions.
- Sigils evaluate by part-level unwrap: when a part holds
  `SigiledTypeExpr(inner)`, the dispatch driver recursively
  dispatches `inner` through the standard classifier (no new shape,
  no separate type-context table). The sigil boundary asserts the
  returned `KObject` is a type-side carrier (`KTypeValue`, `Module`,
  `Signature`, `UserType`, `KFunctor`); value-side carriers in sigil
  position are errors at the boundary, including
  `TypeConstructorCall` shapes that construct value-side instances.
  Existing classifier arms serve the inner expression unchanged: the
  current `TypeCall` arm is the fallback for positional inputs
  (`[Type(List), Type(Number)]`) the parser no longer folds, and
  `Keyworded` runs the new keyworded overloads (`LIST OF`,
  `MAP _ -> _`, etc.).
- User-defined functors are invocable through the sigil identically to
  builtin parameterized types. `:(MyFunctor (T = IntOrd))` and
  `:(LIST OF Number)` route through the same machinery.
- Fully-uppercase head keywords (`LIST`, `MAP`, `FN`) keep
  parameterized-type construction in narrow candidate buckets, so
  user-defined functors overloading short connector words like `OF`
  don't pay a bucket-walk cost on every dispatched parameterized type.

**Directions.**

- **Sigil-marker representation in the AST — decided:
  `ExpressionPart::SigiledTypeExpr(Box<KExpression>)`.** The parser's
  job is to mark "this slot evaluates to a type" — *not* to recognize
  what shape the inner expression takes. Every `:(...)` emits this
  variant wrapping the raw `KExpression`; shape recognition is the
  dispatcher's responsibility. The variant carries the type-context
  through splicing and lifting and is exhaustive-match-checked by the
  compiler. Alternatives rejected: a context-flag boolean on
  `KExpression` (fragile against missed propagation, invites flag
  creep), and a separate `KSigiledExpr` AST node with a parallel
  pipeline (defeats the unification principle).
- **`Dict` → `MAP` surface rename — decided.** Underlying type
  identity stays `KType::Dict(K, V)`; only the surface keyword
  changes.
- **Connector keywords — decided.** `LIST OF`, `MAP _ -> _`. Fully
  uppercase per the bucket-collision rationale in `Impact`.
- **Function-type slot surface — decided: named at the surface,
  positional in `KType` identity for this PR.**
  `:(FN (x :Number, y :Str) -> Bool)` declares parameter names,
  symmetric with the FN declaration form. Lowering drops the names:
  `KType::KFunction { args, ret }` keeps its positional storage and
  structural equality, so `:(FN (a :Number) -> Bool)` is
  identity-equal to `:(FN (b :Number) -> Bool)`. Promoting names into
  identity (and the structural-equality break that goes with it) is
  the follow-up [`fn-named-identity`](../type_language/fn-named-identity.md).
- **Functor-type slot surface — decided: named at the surface,
  positional in `KType` identity.** Symmetric with functions per
  the rule above: `:(FUNCTOR (T :SomeSig) -> Module)`. Same lowering
  rule — `KType::KFunctor { params, ret }` stores params positionally
  this PR; named identity moves with the FN follow-up.
- **User-functor application shape — decided: nested-parens kwarg
  group, symmetric with value-side function-value calls.**
  `(MyFunctor (T = IntOrd))` value-side and `:(MyFunctor (T = IntOrd))`
  sigiled. One rule across both surfaces: head + one nested-parens
  part holding the kwargs, matching
  [`execution-model`](../../design/execution-model.md)'s
  `FunctionValueCall` shape.
- **`TypeCall` classifier variant — decided: keep as the positional
  fallback.** When the part-evaluation unwrap recursively dispatches
  the inner expression of a sigil, the `TypeCall` arm +
  `resolve_type_expr` serve positional inputs (`:(List Number)` →
  `[Type(List), Type(Number)]` after the parser stops folding). New
  keyworded inputs (`:(LIST OF Number)`) go to `Keyworded`. Deleting
  `TypeCall` is a follow-up that depends on every annotation in the
  tree migrating to the keyworded form.
- **Migration strategy — decided: single PR.** The parser change is
  uniform (no shape recognition; every `:(...)` emits
  `SigiledTypeExpr`). The sigil's inner expression dispatches through
  unchanged classifier arms — `TypeCall` for positional inputs,
  `Keyworded` for keyworded — and the sigil boundary asserts the
  result is a type-side carrier. Source annotations don't need
  rewriting for correctness; they keep working through the `TypeCall`
  fallback. Migrating annotations to the keyworded form and deleting
  the `TypeCall` fallback are separate, optional follow-ups. New
  keyworded shapes get dedicated tests in this PR.

## Dependencies

**Requires:**
- [Unified walk + strict-only admission](unified-walk.md) — the
  dispatch substrate this work builds on (candidate buckets,
  binder admission, classifier). Parking Phase 2 of the fast-lane
  subsume in favor of this work keeps the classifier surface
  consistent across the two efforts; this item ships first, then
  Phase 2 of the fast-lane subsumption resumes with the
  `TypeConstructorCall` shape and the sigiled-Keyworded routing
  landing together.

**Unblocks:**
- Phase 2 of [`scratch/plan-fast-lane-subsume.md`](../../scratch/plan-fast-lane-subsume.md) —
  the value-side `TypeConstructorCall` arm of the classifier ships
  alongside the sigiled-Keyworded arm so the type-language and
  value-language surfaces stay symmetric.
- [FN/FUNCTOR named identity](../type_language/fn-named-identity.md) —
  promotes parameter names from the sigil surface this item
  introduces into `KType::KFunction` / `KType::KFunctor` identity, so
  function-typed slots enforce the declared names mechanically.
