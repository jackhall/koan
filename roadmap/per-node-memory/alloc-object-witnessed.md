# `alloc_object` returns `Witnessed`

Migrate the object allocation family onto `yoke`, so every `KObject` born in a per-call region
comes back already bundled with the set of regions it reaches.

**Problem.** [`region.alloc_object`](../../src/machine/core/arena.rs) (~25 call sites) returns a
bare `&'a KObject` that is not witnessed at all: the co-location invariant — that the witness pins
*this* value's references — stays implicit in the region machinery, and the transitional bare path
bundles it with a *prose-asserted* `Witnessed::new` over an over-approximate witness rather than the
exact reach a constructed carrier would name, even though the [`Witnessed::yoke`](../../src/witnessed.rs)
/ `merge` constructors and the production witness plumbing now exist. The regions an object reaches are not named on its carrier at all; two
transitional mechanisms stand in. The consumer-pull lift drops each dep to a bare
[`Carried`](../../src/machine/model/values/carried.rs) at the
[`relocate`](../../src/machine/execute/run_loop.rs) — *discarding* the dep `Sealed` carrier's witness
set, which it held a moment earlier — then re-derives the reach per-value from a surviving
reference's scope `region_owner` ([`reached_frame`](../../src/machine/execute/lift.rs)) and
accumulates it on the consumer frame ([`FrameStorage.retained`](../../src/machine/core/arena.rs)). So
a read-out recovery and a frame-level accumulator together reconstruct, after the fact, a reach the
carrier could have named at construction.

**Acceptance criteria.**

- `alloc_object` returns a `KObject` bundled with the set of regions it reaches, built inside the
  witness closure: region-pure parts via `yoke`, an embedded owned splice-free expression (a quoted
  expression, an FN body) via `yoke` (`alloc_witnessed_embedding`), and dep element carriers or a
  captured scope folded in via `merge`. A region-resident object is born co-located by construction,
  its reach named on the carrier — co-location enforced by the `for<'b>` brand, not asserted.
- The object family carries no `Witnessed::new`: every construction is `yoke` (region-pure parts, an
  embedded owned splice-free expression) or `merge` (witnessed deps, a captured scope, a single
  embedded dep / bound value), never an arbitrary value paired with an asserted witness.
- A construction finish receives its deps as witnessed carriers, not bare `Carried`: the
  consumer-pull lift hands each dep's witness set through to the construction site so `merge`
  composes it, rather than discarding it at the `relocate`.
- A lifted object's reached regions are read off its carrier's witness set: the object path no longer
  *depends on* the read-out `reached_frame` recovery or the `FrameStorage.retained` accumulator to
  reconstruct reach — the carrier names it end-to-end. (The two mechanisms are not deleted here:
  `KType::Module` still rides them on the type channel, its child scope living in a region its own slot
  witness does not name. [`alloc_ktype`](alloc-ktype-witnessed.md) converts that last user and deletes
  both.)
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the shipped substrate — decided.* The production `WitnessRegion` / `MergeWitness` impls,
  the unified `FrameSet` set-witness, the `transfer_into` relocation verb, and the per-value frame
  anchor's removal shipped (a stored value now holds no owning `Rc` back to a region, so the engine
  needs no cycle gate; see
  [memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure));
  this item is the object-family conversion over that foundation.
- *Construction inversion, not post-hoc bundling — decided.* The object is built inside the witness
  closure — `yoke` for region-pure parts, `merge` for dep element carriers — so its reach is named on
  the carrier, not asserted after the fact. Where a `for<'b>` closure cannot reproduce the
  construction (it captures a borrow that is neither region-derived nor owned / `'static`), the value
  is adopted under its exact `singleton(F)` via *structural* `Witnessed::new`, never the prose-asserted
  bundle — a `for<'b>` closure cannot accept an already-built `KObject<'a>`.
- *AST-embedding and single-dep sites yoke or `merge`, not `new` — decided.* An FN body and a quoted
  expression are *owned* [`KExpression`](../../src/machine/model/ast.rs) clones, not `&'run` AST
  references (`require_kexpression` clones; `Body::UserDefined` / `KObject::KExpression` own their
  expression). A raw, unevaluated expression is splice-free, so its sole `'a`-bearing variant
  (`ExpressionPart::Spliced(Carried)`) is absent and it binds no live borrow. So an AST-embedding
  object *yokes* its expression: [`alloc_witnessed_embedding`](../../src/machine/core/arena.rs) moves
  the owned splice-free expression into the `yoke` closure, re-anchors it onto the brand via the
  safe-signature `reattach_with`, and allocs the object natively at the brand — co-location enforced by
  the `for<'b>` brand, the embedded AST contributing no region of its own. `quote` is converted;
  `alloc_function` reuses the same primitive when it moves off the bare `Action::Done(Ok(Carried))`
  channel. `catch`'s `Tagged` and the `Wrapped` newtype each embed a single dep / bound value and
  `merge` that one carrier (a single-dep fold). None carries a `Witnessed::new`.
- *The set is built at construction, not recovered after — decided.* `yoke` / `merge` / structural
  `new` at the alloc site builds the reached-region set directly: a closure's witness is its captured
  scope's defining frame (`scope.region_owner()`); an aggregate's is its elements' carrier sets,
  flattened by `merge` (a closure capturing closures branches into independent lineages — the reach
  is a tree, folded into one set). The `region_owner` walk survives **only** as this construction-site
  witness source; the standalone read-out `reached_frame` recovery is retired by
  [`alloc_ktype`](alloc-ktype-witnessed.md), once `KType::Module` is the last value still riding it.
- *Operands ride the carriers already in hand — no new channel — decided.* The dep `Sealed` carriers
  the consumer-pull lift already opens carry the sets; the plumbing **keeps** them to the
  construction finish instead of discarding them at the `relocate`. Nothing new threads through
  `Carried` / `Held` / `ArgValue`, and scope bindings stay bare — a capturing container `merge`s the
  scope's frame `{F}`, and because `F` *owns* its reached set, holding `{F}` transitively pins
  everything bound into that scope, so the scope needs no per-binding witness. This is the substrate's
  composition law (one wrapper per node): `merge` folds the tree of lineages into one carrier set, so
  the reach is never stacked `Witnessed`-in-`Witnessed` and never duplicated in a side accumulator.
- *`frame.retained` retires together with `reached_frame` — decided.* The accumulator's only job is
  covering the window between the set-dropping `relocate` and the construction that rebuilds the
  reach; once every carrier names its reach end-to-end it is redundant. It becomes redundant for the
  object path here, but is deleted with `reached_frame` by [`alloc_ktype`](alloc-ktype-witnessed.md),
  whose `KType::Module` is the last value whose slot witness does not already name its own reach (see
  [per-node-memory.md § Transfer](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into)).
- *Lands the shared dep-result plumbing — decided.* The "keep the dep set to the finish" wiring and
  the carrier-reads-its-own-reach read-out boundaries are family-agnostic, so they land here and the
  [type family](alloc-ktype-witnessed.md) builds on them. The value-read side
  ([value reads](value-reads-to-open.md)) is orthogonal: a copy-out read never touches the set, and
  an escaping read re-anchors through `transfer_into`, which now reads the reach off the carrier.

## Dependencies

This item lands the shared dep-result plumbing (the lift hands each finish its deps' carriers) that the
type family reuses. Its aggregate and leaf inversions build on the shipped substrate (`yoke` / `merge`
/ `transfer_into`); its AST-embedding inversions (`alloc_function`, `quote`) yoke an owned splice-free
expression via the shipped [`alloc_witnessed_embedding`](../../src/machine/core/arena.rs).

**Requires:** none — foundation (the witnessed substrate, including the AST-embedding `yoke`, is shipped).

**Unblocks:**

- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — reuses the dep-result plumbing this
  lands, and completes the `reached_frame` / `FrameStorage.retained` deletion by taking the last
  `KType::Module` user off them.
