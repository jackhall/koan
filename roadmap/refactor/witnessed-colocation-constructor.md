# Co-location-enforcing `Witnessed` constructor

Bundle a value with its liveness witness through a constructor that *obtains* the value from the
witness's own region, so the witness-pins-the-value relationship is structural rather than
caller-asserted.

**Problem.** [`Witnessed::new(value, witness)`](../../src/witnessed.rs) bundles an *arbitrary* value
with an *arbitrary* witness. The [`Witness`](../../src/witnessed.rs) trait's safety contract — that
"the storage the carrier's erased pointee refers to stays live" for as long as the witness is held — is
**caller-asserted and unchecked**: nothing verifies that the witness actually pins *this value's*
references (the co-location invariant). A value bundled with a witness that pins a *different* region
type-checks and runs, keeping the wrong region alive while the referenced data dies — a use-after-free
the type system cannot catch. `Witnessed` does enforce witness *liveness* (it owns the `Rc`) and bounds
*access* (the rank-2 `for<'b>` brand on [`with`](../../src/witnessed.rs) / [`map`](../../src/witnessed.rs)
keeps a re-anchored borrow from escaping the pin), but **not** that the witness pins the bundled value.
So the co-location invariant rides as a prose SAFETY note at every `Witnessed::new` and
[`reattach_with`](../../src/witnessed.rs) site (≈ a dozen, across `node_store.rs`, `runtime.rs`,
`lift.rs`, `dispatch/ctx.rs`, …), and a wrong-witness pairing is a latent memory-safety bug rather than
a compile error.

**Acceptance criteria.**

- A constructor obtains the carrier *from the witness's own region* through a rank-2 (`for<'b>`)
  closure — e.g. `cart.yoke(|region: &'b Region<'b>| -> T::At<'b>) -> Witnessed<T, Rc<Cart>>` — so the
  only references the produced value can hold are ones reached through that region: co-location holds
  **by construction**, discharged at the constructor rather than asserted at each call site.
- The `for<'b>` brand forbids the closure from returning a reference captured from its environment (a
  captured `&'x` cannot satisfy `'x: 'b` for every `'b`), so the lifted value's references are
  region-derived or owned / `'static` — never a smuggled foreign borrow. A `compile_fail` test pins
  this, mirroring the existing guards on [`Witnessed::with` / `map`](../../src/witnessed.rs).
- Scope- and region-carrying values are bundled through the enforcing constructor; the free
  `Witnessed::new` is reserved for carriers whose co-location is already structural (lifetime-free
  carriers), or is removed where the enforcing form subsumes it.

**Directions.**

- *Witness granularity — open.* Whether the closure is handed a region (`&'b Region<'b>`) or a scope
  (`&'b Scope<'b>`, the interior-mutable binding-table case), and whether the bundled witness is the
  region `Rc` or the cart `Rc<CallFrame>` (which pins the region *plus* its `outer` chain).
  Recommended: a generic `yoke` over the witness's owned-region accessor, with a `Scope`-specialized
  wrapper for the mutation sites.
- *Relationship to `map` — decided.* The enforcing constructor is the build-time twin of
  [`Witnessed::map`](../../src/witnessed.rs): both run a `for<'b>` closure and re-seal the result to
  `'static` storage, but where `map` transforms an *already-bundled* carrier, this **sources** the
  carrier from the witness's region — closing the co-location gap `new` leaves open.
- *Adoption scope — open.* Whether to migrate every `Witnessed::new` scope/region site, or only the
  new lift / continuation paths that motivate the constructor.

## Dependencies

Extends the consolidated [`witnessed` carrier module](witnessed-carrier.md) — the `Reattachable` /
`Erased` / `with` / `map` machinery this constructor builds on.

**Requires:** none — engine-internal witness machinery.

**Unblocks:**

- [FrameStorage self-reference removal](framestorage-self-reference.md) — its continuation rework must
  lift scope-carrying values across the step boundary, which is sound only through a co-location-enforced
  bundle.
