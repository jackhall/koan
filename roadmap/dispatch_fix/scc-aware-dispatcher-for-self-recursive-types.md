# SCC-aware dispatcher for parameterized self-recursive types

Plumb the elaborator's threaded-set and current-declaration context
into the standalone dispatcher so a parameterized self-reference inside
a `STRUCT` / `UNION` / `SIG` body short-circuits to `RecursiveRef`
rather than failing `UnboundName`.

**Problem.** The
[type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md)
substrate routes every sigiled type expression through the dispatcher
— a parameterized form like `:(LIST OF Tree)` sub-Dispatches the inner
expression, hits the registered `LIST OF` overload, and resolves the
`Tree` slot through the dispatcher's bare-Type-leaf fast lane. That
fast lane has no idea it's running inside `STRUCT Tree`'s own body:
the [`Elaborator`](../../src/machine/model/types/resolver.rs)'s
threaded set (`with_threaded`) and current-declaration context
(`with_current_decl`) — which let the inline field-walker
short-circuit `Tree` → `KType::RecursiveRef("Tree")` — never reach the
standalone dispatcher. Consequently a self-reference like
`:(LIST OF Tree)` reaches `bare_type_leaf`, looks up `Tree`, finds no
finalized binding (the binder hasn't returned yet), and surfaces
`UnboundName`.

The field-walker in
[`typed_field_list`](../../src/machine/model/types/typed_field_list.rs)
retains an inline `try_synth_legacy` arm to keep legacy positional
sigil shapes (`:(List Tree)`) elaborating with full SCC context for
embedded STRUCT/UNION field schemas. Keyworded sigils (`:(LIST OF
Tree)`) and standalone parameterized types in any other slot route
through the standalone dispatcher and lose the threading. Two
test ignores pin the gap:

- `recursive_struct_tree_elaborates_with_recursive_ref_on_field` in
  [`src/builtins/struct_def/tests/recursion.rs`](../../src/builtins/struct_def/tests/recursion.rs)
- `fn_with_invalid_list_arity_errors_at_definition` in
  [`src/builtins/fn_def/tests/container_types.rs`](../../src/builtins/fn_def/tests/container_types.rs)
  — the legacy positional `:(List X, Y)` arity error no longer
  surfaces because the input doesn't route through
  `KType::from_type_expr`'s arity check.

**Impact.**

- Parameterized self-recursive types declare cleanly through the
  dispatcher: `STRUCT Tree = (value :Number, children :(LIST OF
  Tree))` elaborates `children` as
  `KType::List(Box::new(KType::RecursiveRef("Tree")))` and finalizes
  without parking on its own placeholder.
- The field-walker's inline `try_synth_legacy` path retires: every
  type-language elaboration routes through the dispatcher, so the
  field-walker uses the same code path as any other type-position
  slot. The `TypeParams::List | Function` arms in
  [`make_capture`](../../src/machine/model/types/resolver.rs) and
  the resolver's positional-fold helpers become dead code at the
  same time.
- Re-enables the four ignored tests across `struct_def`, `fn_def`,
  and `type_ops/type_constructor` modules — `STRUCT Tree`'s recursive
  field, FN's container-type arity validation, and user-defined
  `TypeConstructor` round-trips through SIG / FN slots.

**Directions.**

- **Threading carrier — open.** Two candidates: (a) extend
  `Dispatch` node's submission-time context with optional
  `(threaded: &HashSet<String>, current_decl: Option<&str>)` fields
  that propagate through `sub_Dispatch` and `Combine`; (b) introduce
  a per-scope register the dispatcher consults on each bare-Type
  leaf. *Recommended: (a).* The SCC context is per-elaboration, not
  per-scope — `STRUCT Tree`'s body is the only place `Tree` should
  short-circuit to `RecursiveRef`, and that scoping is exactly what
  a context attached to the submission point provides. (b) leaks the
  recursion context into unrelated dispatches sharing the same
  scope.
- **Bare-Type-leaf integration — decided per (a).** The
  `fast_lane_bare_type_leaf` handler consults the carried context
  before falling through to `Scope::resolve_with_chain`: if the leaf
  name matches `current_decl` or is in `threaded`, return a
  `KType::RecursiveRef(name)` carrier directly.
- **Sub-Dispatch propagation — decided per (a).** Every internal
  sub-Dispatch issued by the dispatcher (sigil-tail-replace,
  `Combine` wake, `Bind` re-dispatch) carries the same context. The
  binder's *body* dispatches do NOT inherit — they enter their own
  scope and re-establish the threaded set as the elaborator does
  today.
- **Field-walker retirement — open.** Two paths: (i) delete
  `try_synth_legacy` and route every sigil through the dispatcher
  uniformly; (ii) keep the inline path for performance (avoids one
  sub-Dispatch per field). *Recommended: (i).* The sub-Dispatch cost
  is a single bind/finalize per field; uniformity beats the
  microoptimization, and retiring the legacy arm lets
  `KType::from_type_expr`'s `TypeParams::List | Function` arms die
  too.
- **`TypeParams::List | Function` prune — deferred.** Same PR or a
  separate prune item — once `try_synth_legacy` is gone, the parser
  no longer produces these `TypeParams` variants, so the resolver
  and `make_capture` arms that handle them become dead code. The
  prune is a mechanical follow-up: grep for the variants, delete the
  arms, run tests.

## Dependencies

**Requires:**

- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md)
  (shipped) — the substrate that routes parameterized type
  construction through the dispatcher and surfaced the SCC-context
  gap.

**Unblocks:** none directly; closes the field-walker / dispatcher
split and retires the inline `try_synth_legacy` arm plus the
`TypeParams::List | Function` resolver / `make_capture` arms that
become dead code with it. The
[user-defined TypeConstructor keyworded
application](user-defined-typeconstructor-keyworded-application.md)
item composes with this work for self-recursive user-functor
declarations but ships independently — the non-recursive
declaration shape works without SCC threading.
