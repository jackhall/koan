# Structural witnesses

Every carrier witness is computed — by `yoke`, `merge`, or a carrier retained at its
bind site — never asserted by pairing an already-built value with a hand-supplied
witness.

**Problem.** [`Witnessed::new`](../../src/witnessed.rs) bundles a value with a witness the
constructor cannot check: co-location is stated in prose at each call site, not enforced.
Six live sites in three categories survive the per-node-memory project:

- *The object resident read.* [`Scope::resident_object_carrier`](../../src/machine/core/scope.rs)
  wraps an already-built `&'a KObject` as `Witnessed::new(.., FrameSet::singleton(home))` —
  six callers: FN def ([`fn_def/finalize.rs`](../../src/builtins/fn_def/finalize.rs)), name
  resolution ([`scope.rs`](../../src/machine/core/scope.rs)), ATTR member access
  ([`attr.rs`](../../src/builtins/attr.rs), two sites), the LET object RHS
  ([`let_binding.rs`](../../src/builtins/let_binding.rs)), and the literal Resolved arm
  ([`dispatch/literal.rs`](../../src/machine/execute/dispatch/literal.rs)). The read re-wraps a
  bare reference because [`Bindings`](../../src/machine/core/bindings.rs) stores `data` as
  `(&'a KObject, BindingIndex)` and `types` as `(&'a KType, BindingIndex)` — no carrier, so a
  lookup has no reach to return and asserts a single-frame one instead.
- *The bare-`Done` terminal forward.* [`finalize_terminal`](../../src/machine/execute/finalize.rs)
  wraps a live `Carried` under a separately-computed `dep_reached ∪ producer` set. Every node
  whose step is `NodeStep::Done` (not `DoneWitnessed`) routes here, so the asserted bundle is the
  witness point for the entire non-construction terminal class.
- *The `RegionTypeFamily` operand bundles.* The newtype / tagged-union constructors
  ([`dispatch/constructors.rs`](../../src/machine/execute/dispatch/constructors.rs), three sites)
  and the `CATCH` `Result` build ([`catch.rs`](../../src/builtins/catch.rs)) pair a destination
  `RegionBrand` with a foreign `&KType` identity via `Witnessed::<RegionTypeFamily, FrameSet>::new`,
  asserting the identity's region is pinned by the dest frame's `outer` chain.

Separately, [`WitnessRegion for FrameSet`](../../src/machine/core/arena.rs) returns
`frames.first()` and **panics on an empty set**: the trait models one canonical region, but
`FrameSet` is a set, so `yoke`'s single-region precondition is held by a runtime narrowing, not a
type. A multi-region value witnessed by a single asserted frame — the shape a list of closures over
distinct, independently-dying per-call regions produces — drops the other regions from the pin, a
use-after-free once those frames free. It is latent only because no asserted-witness site is forced
to compute the union, and the test slate has no multi-region case to surface it.

Two residues compound the surface: [`Option<W>: Witness`](../../src/witnessed.rs) is instantiated
nowhere (the substrate and [`arena.rs`](../../src/machine/core/arena.rs) describe the frameless
terminal as `Option`/`None`, but Koan represents it as `FrameSet::empty()`), and
[`Witnessed::read`](../../src/witnessed.rs) is `pub` with a single internal caller (`Sealed::open`).
[design/per-node-memory.md](../../design/per-node-memory.md) and the roadmap README already describe
FN def as *yoking* its `KObject::KFunction` and aggregates as no longer asserted — the intended end
state, which `fn_def/finalize.rs` (the asserted read) has not yet reached.

**Acceptance criteria.**

- `Witnessed::new` does not exist. No carrier is built by pairing an already-built value with an
  independently-supplied witness anywhere in the workload.
- A freshly-built object terminal is born witnessed through one allocate-and-witness operation: FN
  def and the LET object RHS allocate through their scope and seal under that scope's frame in a
  single call, so the registered `&'a` and the returned carrier share one allocation and co-location
  is structural.
- A value read back by name or ATTR member returns the carrier retained at its bind site; `Bindings`
  carries each value binding's reach, and no lookup re-wraps a bare `&KObject`.
- Every node finalizes a witnessed carrier — a region-pure result through the empty-set resident
  path, a dep-reaching result by folding its delivered dep carriers — so `finalize_terminal`'s
  asserted bundle, the `dep_reached: FrameSet` threading, and the `NodeStep::Done` / `DoneWitnessed`
  split collapse to one witnessed terminal.
- The `RegionTypeFamily` operand is assembled by `yoke`ing the destination brand and `merge`ing a
  delivered type-identity carrier; the nominal identity crosses the build brand witnessed by its own
  region, not asserted.
- `WitnessRegion` is implemented only for a single-region witness type; `yoke` accepts that type and
  cannot be called with an empty or multi-region witness. `FrameSet` exposes no panicking `region()`
  — the single-region precondition is a compile-time constraint, not a runtime `expect`.
- `Witnessed::read` is private to the `witnessed` module.
- `Option<W>: Witness` is deleted; the frameless terminal is `FrameSet::empty()` in code and in the
  `witnessed.rs` / `arena.rs` prose, with no `Option`/`None` witness narrative remaining.
- A Miri test covers each multi-region shape — a list of closures over distinct, independently-dying
  per-call regions; a closure capturing closures across several regions (the reach tree); a record or
  dict whose values reach distinct regions — and asserts a read after the producing frames free
  touches no freed memory. Each is recorded in the audit slate ([TEST.md](../../TEST.md)) and fails if
  a construction path witnesses the aggregate by a single under-counting frame.
- The only lifetime retypes remain the substrate's audited `retype` / `with_branded_ref`
  (`witnessed.rs`, `region.rs`); no new asserted-witness or `transmute` path is introduced.

**Directions.**

- *Single-region yoke witness — decided.* Introduce a single-owner witness — a newtype over
  `Rc<FrameStorage>` — that impls `WitnessRegion`, leaving `FrameSet` with only `Witness +
  MergeWitness`, and route `yoke` / `alloc_*_witnessed` through the single-owner type. The alloc
  surface always targets one region, so the single-region precondition lives in the type rather than a
  runtime `region()` that narrows-and-panics on a set.
- *Object read-site carrier — decided.* Store each value binding's reach (its `FrameSet`, or its
  `Sealed` carrier) on the classified [`Bindings`](../../src/machine/core/bindings.rs) entry — the
  value/type split that hangs it is now structural (a value bind and a type bind of one name are
  mutually exclusive by construction), so a name / ATTR lookup returns a witnessed carrier instead
  of re-wrapping a bare `&KObject`.
- *Object define-site — decided.* Move alloc + seal into one `Scope` method, so a freshly-built FN /
  LET-RHS object is witnessed by the scope that allocated it; the bare `&'a` needed for registration
  and the carrier returned to the scheduler come from the same call.
- *Bare-`Done` collapse — decided.* Bare-terminal producers deliver witnessed carriers (region-pure →
  empty-set resident, the producer frame folded at close; dep-reaching → fold the delivered dep
  carriers), retiring `finalize_terminal` and `NodeStep::Done` and replacing the `dep_reached`
  threading with dep-carrier folding.
- *Type-identity carrier delivery — decided.* The nominal identity in `CtorKind` (and `catch`'s
  `Result` build) rides a delivered type carrier so the operand is `merge`d, not asserted; reuses the
  type channel's existing `seal_type` delivery.
- *`Option<W>: Witness` removal — decided per user.* Delete the impl; `FrameSet::empty()` is the
  frameless terminal; rewrite the `Option` / `None` narrative in `witnessed.rs` and `arena.rs`.
- *`Witnessed::read` visibility — decided.* Private to the module.

## Dependencies

**Requires:**

**Unblocks:** none tracked yet.
