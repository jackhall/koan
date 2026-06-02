# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
[roadmap/](roadmap/).

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](design/) — five
topical docs covering the execution model, memory model, functional programming,
expressions and parsing, and error handling, plus [design/typing/](design/typing/README.md)
covering the type and module systems end-to-end.

What's shipped that the open items below build on:

- *Module language.* `MODULE` / `SIG` declarators, `:|` / `:!` ascription, `SIG_WITH`
  sharing constraints, higher-kinded type-constructor slots (declared with `TEMPLATE`,
  applied as `:(arg AS Ctor)`), and the type-language
  collapse that puts modules and signatures in `KType` directly via `KType::Module`,
  `KType::Signature`, and `KType::AbstractType` carriers (the last rooted at either a
  SIG-declared member or a per-call opaque mint via `AbstractSource`). An opaque view's
  VAL-slot read re-tags through the `Wrapped` carrier so it reports the abstract member
  identity rather than the underlying representation. Values carry runtime
  type-parameter carriers, stamped at FN return, argument, and `LET` boundaries.
- *Block-scoped module opening.* `USING … SCOPE` surfaces a module value's members as
  bare names for the duration of a block, splitting reads and writes across the
  transparent-scope `outer` chain.
- *FUNCTOR binder.* A dedicated `FUNCTOR` binder with its `:(FUNCTOR (params) -> R)`
  type-position sigil and the one-way `KFunctor` / `KFunction` admissibility wall.
- *Type language via dispatch.* The `:(...)` sigil is a parse-context marker
  emitting `ExpressionPart::SigiledTypeExpr(Box<KExpression>)` with no inner
  shape-folding; the dispatcher's `SigiledTypeExpr` fast lane tail-replaces
  the slot with a `Dispatch` of the wrapped expression. Keyworded
  overloads — `LIST OF`, `MAP _ -> _`, `FN`, `FUNCTOR` — register in
  `builtins/type_constructors.rs` and serve every fresh parameterized-type
  annotation. The submission walk reifies the binder install channel as
  `BinderKey::Name` (`LET` / `STRUCT` / `UNION` / `SIG` / `MODULE`) vs.
  `BinderKey::Bucket` (`FN` / `FUNCTOR`), and `pending_overloads` carries a
  per-bucket Vec so sibling FN / FUNCTOR overloads coexist as distinct
  wake sources with earliest-index-visible parking. A self-reference inside
  a keyworded field sigil (`STRUCT Tree = (children :(LIST OF Tree))`) is
  pre-resolved to a `RecursiveRef` carrier by `rewrite_threaded_self_refs`
  before the sub-Dispatch, so it lowers to `List(RecursiveRef("Tree"))`
  instead of deadlocking on its own placeholder.
- *Type-only nominal identities.* `STRUCT` / `UNION` / `MODULE` / `Result`
  declarations write only `bindings.types`: each per-declaration
  `KType::UserType` identity carries its own schema payload
  (`UserTypeKind::Struct { fields }`, `Tagged { schema }`,
  `TypeConstructor { schema, param_names }`, alongside the existing
  `Newtype { repr }`), and construction reads that schema from the type
  entry rather than a value-side carrier. The `KObject::StructType` /
  `TaggedUnionType` carrier variants are gone, so `bindings.data` holds
  only runtime instances; value-position references synthesise
  `KTypeValue(identity)` on demand via `coerce_type_token_value`, and
  recursive types ride a cycle-close pre-install plus a schema-bearing
  upsert at finalize. `SIG` followed the same path by merging its
  constraint variant (`SatisfiesSignature`) and value variant
  (`Signature(s)`) into one `KType::Signature { sig, pinned_slots }` —
  disambiguated by position — so it writes a single type-side identity and
  the `register_nominal` / `try_register_nominal` / `derive_nominal_identity`
  machinery deleted. No nominal binder dual-writes; the type-language /
  value-language partition is total.
- *TypeName carrier collapse.* The parser's bare type-leaf carrier is a
  `TypeName(String)` newtype (`Deref` to `str`, derived eq/hash) in place of the
  old `TypeExpr` struct, so the `ExpressionPart::Type` / `KObject::TypeNameRef`
  variants carry the name directly. Dropping `TypeExpr`'s per-token
  `OnceCell` builtin cache removed the `KType<'static>` → `KType<'a>` transmute
  in `resolve_for` (one fewer unsafe site), leaving the scope-bound
  `type_expr_memo` as the sole cache tier; bind-time builtin lowering re-runs the
  `from_type_expr` → `from_name` match per call. The three leaf-resolution
  contexts (`elaborate_type_expr`, `coerce_type_token_value`, `resolve_for`) stay
  distinct but share one `resolve_type_with_chain` + `from_name` lookup.
- *Record substrate.* [`Record<V>`](src/machine/model/types/record.rs) — an ordered
  identifier-keyed map with order-blind equality and a commutative name+value hash
  (`wrapping_add` fold over `mix(hash(name), hash(value))`) — backs the struct schema
  (`UserTypeKind::Struct` carries `Rc<Record<KType>>`) and FN/FUNCTOR parameter
  identity (`KType::KFunction` / `KFunctor` carry `params: Record<KType>`, so parameter
  names round-trip through `KType::name()` and slot identity is order-blind by name and
  type; the retired `FUNCTION_OF` builtin gives way to the `:(FN …)` carrier). `KType`
  gained a hand-written `Hash` consistent with its manual `PartialEq`, so a record type
  can key a dispatch / memo map. The dict carrier stays a sibling. See
  [design/typing/ktype.md](design/typing/ktype.md#record-fields-and-ktype-hashing).
- *Function subtyping.* `function_compat` and `is_more_specific_than` admit and rank
  function-typed slots by sound function subtyping — contravariant parameter records
  with width-drop, covariant return — instead of strict structural equality. A value
  `(x :Any) -> Str` fills a `:(FN (x :Number) -> Str)` slot, a unary value fills a binary
  slot (the surplus slot param arrives unbound under call-by-name, and the call-by-name
  binder drops surplus named args), and incomparable function slots now tie as
  `AmbiguousDispatch`. See
  [design/typing/ktype.md § Variance](design/typing/ktype.md#variance).
- *Structural records.* A first-class `KType::Record(Record<KType>)` type
  (`:{x :Number, y :Str}`) and anonymous record value (`{x = 1, y = "a"}` — `=` pairs;
  `:` pairs stay a dict, mixing them is a parse error), with width/depth subtyping: a
  wider record value is more specific (the dual of function-param width-drop), depth
  covariant (sound under value immutability), so record-typed FN overloads dispatch by
  width and depth and incomparable arms tie as `AmbiguousDispatch`. Structs stay
  nominal; the record variant is structural-only. The
  [`FROM` projection](src/builtins/record_projection.rs) closes the projection direction:
  `(x y) FROM r` re-tags a record value's carried type to the named fields (`Rc`-sharing
  the backing record whole) so a caller can break an incomparable-arm tie. See
  [design/typing/ktype.md § Variance](design/typing/ktype.md#variance).
- *Named-argument surface.* The record literal `{x = 1}` is the sole named-argument form
  across struct construction (`Point {x = 1, y = 2}`), function-value calls (`f {x = 1}`),
  and functor application (`:(MyFunctor {T = IntOrd})`); the `(x = 1)` paren-kwarg and
  `{x: 3}` dict forms are retired, and an empty `{}` is the empty record so a nullary call
  spells `f {}`. The dispatch lanes classify a call body as named (record literal) or
  positional (paren group — tagged-union / newtype construction) via `extract_call_body`,
  and the resolved [`ArgumentBundle`](src/machine/core/kfunction/argument_bundle.rs) carries
  its arguments on the same [`Record<V>`](src/machine/model/types/record.rs) shape as the
  surface literal and the struct value. See
  [design/typing/type-language-via-dispatch.md](design/typing/type-language-via-dispatch.md).
- *In-walk dispatch precedence.* Overload resolution decides each scope's park / defer /
  pick contribution at the scope that raised it instead of in a scope-blind post-walk
  ladder, so lexical shadowing holds regardless of finalize or evaluation order: a visible
  pending sibling parks its scope even over a same-scope finalized strict-Pick, and a
  strict-Empty bucket runs one relaxed-admission pass per candidate (parked-lean ⇒ park,
  eager-lean ⇒ defer, dead unbound lean ⇒ a held-back `UnboundName`). `lookup_function`
  surfaces finalized overloads and the earliest visible pending producer together. See
  [design/typing/scheduler.md § In-walk dispatch precedence](design/typing/scheduler.md#in-walk-dispatch-precedence).
- *Branch-arm return contract.* `MATCH <v> -> :T WITH (...)` and `TRY (<e>) -> :T WITH (...)`
  carry a mandatory declared return type every arm agrees on. The generalized
  [`ReturnContract`](src/machine/core/kfunction/body.rs) slot carrier (`Function(&KFunction)`
  for a call, `Arm { ret, kind }` for a function-less arm) routes both FN and MATCH / TRY
  through the one Done-arm check, so the selected arm's value is runtime-checked against `T`
  (a `<return>` `TypeMismatch` on a miss) and re-tagged to `T` for downstream dispatch. This
  closes the divergent-result hazard symmetric to the divergent-bind closure of the
  lexical-provenance chain. See
  [design/execution-model.md § Arms as own blocks](design/execution-model.md#arms-as-own-blocks).
- *Operator-chain substrate.* Pure-symbol tokens that aren't builtin compound triggers
  classify as keywords, and [`KExpression`](src/machine/model/ast.rs) caches a
  `DispatchShape` at parse time — including an `OperatorChain` track for the slot-led
  `Slot (Keyword Slot)+` shape, with its sorted-joined operator probe. A per-scope
  operator registry (`Bindings::operators`, walked by
  `Scope::resolve_operator_group_with_chain` like every other name) resolves a chain's
  probe to a shared `OperatorGroup`, and the `OperatorChain` dispatch arm hits that
  registry — missing cleanly on an undeclared or cross-group mix, or reaching the fold
  seam on a hit. The fold itself and the `OP` declaration surface are the remaining
  open work under
  [user-definable n-ary operators](roadmap/libraries/n-ary-operators.md) and
  [user-defined operator modules](roadmap/libraries/user-defined-operator-modules.md).
  See [design/expressions-and-parsing.md § Structural cache and dispatch shape](design/expressions-and-parsing.md#structural-cache-and-dispatch-shape).

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Files and imports](roadmap/libraries/files-and-imports.md) — wire `.koan` files together so
  a codebase can span more than one source file and files become modules.
- [Group-based operators](roadmap/libraries/group-based-operators.md) — paired `+`/`-`-style
  operators as a group; the syntax-level shorthand variant has no hard prerequisites.

## Open items

Each subdirectory of [roadmap/](roadmap/) is one project — a coherent body of work
whose items share design constraints and ship together. Per-item write-ups (problem,
impact, directions, dependencies) live in the subdirectory; the summaries below name
what the project buys the language and list its open items.

### Predicate typing — [roadmap/predicate_typing/](roadmap/predicate_typing/)

The user-facing typing stages — axioms, modular implicits, equivalence-checked
coherence, witness types — that ride on top of the type-language substrate.
The agreed design is captured in [design/typing/](design/typing/README.md);
stages 1 and 2 shipped (the module language: `MODULE`/`SIG` declarators,
`:|`/`:!` ascription, per-module type identity, plus the scheduler-driven
elaborator, `SIG_WITH` sharing constraints, and higher-kinded type-constructor
slots, plus runtime type-parameter carriers on `List` / `Dict` / `Result`
values with ascription stamping at the FN return, argument, and `LET`
boundaries):

- [Stage 4 — Property testing and axioms](roadmap/predicate_typing/axioms-and-generators.md)
- [Stage 5 — Modular implicits](roadmap/predicate_typing/modular-implicits.md)
- [Stage 6 — Equivalence-checked coherence](roadmap/predicate_typing/equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](roadmap/predicate_typing/syntax-tuning.md)

### Libraries — [roadmap/libraries/](roadmap/libraries/)

Give Koan a multi-file source surface, an in-language effect/error story, and
a canonical body of Koan code that exercises both. Each item is a piece of
substrate the standard library needs to exist as Koan source rather than as
Rust builtins:

- [Files and imports](roadmap/libraries/files-and-imports.md)
- [Generalize `Scope::out` into monadic side-effect capture](roadmap/libraries/monadic-side-effects.md)
- [Group-based operators](roadmap/libraries/group-based-operators.md)
- [User-definable n-ary operators](roadmap/libraries/n-ary-operators.md)
- [User-defined operator modules](roadmap/libraries/user-defined-operator-modules.md)
- [Standard library](roadmap/libraries/standard-library.md)

### Type language — [roadmap/type_language/](roadmap/type_language/)

Engine-level type-language substrate — how modules, signatures, functors,
deferred-return FNs, record-shaped parameter binding, and VAL-slot identity
are represented in `KType` and routed through dispatch. The substrate the
predicate-typing stages and the stdlib's functor-heavy collections both
build on:

- [Anonymous structural unions](roadmap/type_language/anonymous-unions.md)

### Editor tooling — [roadmap/editor_tooling/](roadmap/editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](roadmap/editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](roadmap/editor_tooling/continue-on-error.md)
