# Collapse TypeExpr and consolidate leaf type-name resolution

Replace the `TypeExpr` struct with a `TypeName` newtype, drop its per-token
builtin cache, and route the leaf-resolution contexts through one shared
`bindings.types` lookup, now that compound type construction routes through
dispatch.

**Problem.** [`TypeExpr`](../../src/machine/model/ast.rs) is the parser's
carrier for a bare type-name leaf (`Number`, `Point`, `T`, `Mo.Ty`), but it
holds only `{ name: String, builtin_cache: OnceCell<KType> }` and its
`PartialEq` / `Hash` key on `name` alone. Compound types (`:(LIST OF Number)`,
`:(FN (x :Number) -> Bool)`) are dispatch expressions over type-constructor
builtins, not `TypeExpr` structure
([resolver.rs](../../src/machine/model/types/resolver.rs): parameterized
shapes "no longer reach this walk"), so the struct carries no information a
name string wouldn't. The two carriers that hold it —
`ExpressionPart::Type(TypeExpr)` ([ast.rs](../../src/machine/model/ast.rs))
and `KObject::TypeNameRef(TypeExpr)`
([kobject.rs](../../src/machine/model/values/kobject.rs)) — keep the
type-position tag in their *variant*, not the inner struct, and the deferral
the struct nominally enables is carried by `ElabResult::Park(Vec<NodeId>)`
(node-keyed parking, independent of `TypeExpr`).

The resolution *around* the carrier is also over-built for what's left after
the structural walk moved to dispatch. Leaf resolution carries two cache
tiers and duplicates the `bindings.types`+`from_name` lookup across three
contexts:

- *Two cache tiers.* `TypeExpr.builtin_cache` (a per-token `OnceCell`,
  scope-independent builtins) and the scope-bound `type_expr_memo` on
  `Bindings` reached through
  [`Scope::resolve_type_expr`](../../src/machine/execute/dispatch/resolve_type_expr.rs).
- *Three lookup contexts.* Declaration-time
  [`elaborate_type_expr`](../../src/machine/model/types/resolver.rs)
  (SCC/threaded-set → `&'a KType`); dispatch-time
  [`coerce_type_token_value`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
  (chain-visibility → `KObject::KTypeValue`); and bind-time
  [`ExpressionPart::resolve_for`](../../src/machine/model/ast.rs)
  (→ `KTypeValue` or the deferred `TypeNameRef`).

The struct and this split thread through ~267 references across ~50 source
files.

**Impact.**

- The AST's type-leaf carrier is a `TypeName` newtype, removing one
  representation from the parse → elaborate → `KType` pipeline
  ([elaboration.md](../../design/typing/elaboration.md)).
- `ExpressionPart::Type` and `KObject::TypeNameRef` carry the name directly;
  the type-position tag stays in the variant, and `TypeName` derives its
  impls in place of the hand-written `Clone` / `PartialEq` / `Hash`.
- The three contexts share one `bindings.types`+`from_name` lookup behind
  thin context wrappers, so the leaf lookup is written once.
- One cache tier instead of two; the per-token `OnceCell` disappears.

**Directions.**

- *Target carrier — decided: a `TypeName(String)` newtype.* A thin newtype
  (`Deref` to `str`) replaces the struct. The `ExpressionPart::Type` /
  `TypeNameRef` variants still tag the position; the newtype makes the
  ~267-site migration compiler-checked — a value-name can't be passed where a
  type-name belongs — and self-documents the type-position role.
- *Builtin lowering — decided: no cache, inline match.* Drop
  `TypeExpr.builtin_cache`;
  [`KType::from_type_expr`](../../src/machine/model/types/ktype_resolution.rs)
  (a match over the ~10-entry builtin table) re-runs per call. A shared
  builtin table is added back only if profiling later shows `resolve_for`
  hot.
- *Scope-bound memo — decided: keep, as the sole cache tier.* The
  `type_expr_memo` and its finalize gate stay — they cache user-type
  resolution (a `bindings.types` walk that must never observe a pre-SCC-close
  identity). Dropping the builtin cache leaves this as the only tier.
- *Leaf-lookup consolidation — decided: shared core, context wrappers.*
  Extract one `bindings.types`+`from_name` lookup. The three contexts wrap
  it rather than duplicate it: declaration adds the SCC/threaded-set and
  returns `&'a KType`; dispatch adds chain visibility and returns
  `KObject::KTypeValue`; bind-time `resolve_for` returns `KTypeValue` or the
  deferred carrier. They do *not* collapse to one function — the contexts
  genuinely differ — but the lookup is written once.
- *TypeNameRef — decided: keep; payload follows the carrier.*
  `KObject::TypeNameRef` stays load-bearing: it carries an unresolved bare
  type-name as a first-class value and is resolved at invoke against the
  *definition* scope ([invoke.rs](../../src/machine/core/kfunction/invoke.rs)),
  which parking does not replace. Its payload becomes the new `TypeName`.
- *Preserve the declaration-time machinery — decided.* The finalize gate
  (no pre-SCC-close identity enters the memo,
  [resolve_type_expr.rs](../../src/machine/execute/dispatch/resolve_type_expr.rs)),
  the SCC cycle-close (`close_type_cycle`), and the `RecursiveRef`
  self-reference threading are load-bearing and stay. This item consolidates
  the carrier, caches, and leaf lookup *around* them — not the
  recursive / forward / mutual type-declaration resolution itself, which
  dispatch does not provide.
- *Migration scope — decided.* Mechanical `TypeExpr` → `TypeName` replace
  across the references; the type/value classification and the parking rails
  are unaffected. Scoped by a grep for `TypeExpr`.

## Dependencies

**Requires:**

- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md)
  (shipped) — routing compound type construction through the dispatcher is
  what left `TypeExpr` degenerate and the leaf-resolution split redundant;
  without it the struct would still carry parameterized structure.

**Unblocks:** none — terminal cleanup.
