# Witnessed carrier module for value lifetime-erasure

Consolidate the value-carrier erase→`'static`→reattach machinery into one top-level module whose
`unsafe` is two audited, independently-tested accessors.

**Problem.** Moving a value along a scheduler dependency edge erases its borrow lifetime to `'static`
for storage and re-anchors it on read. That machinery is scattered: the
[`Reattachable`](../../src/witnessed.rs) trait, the `retype` transmute primitive,
`erase_to_static`, and `Erased<T>` live in `scheduler/erase.rs`, while the `Scope`-specialized reattach
lives in [`scope_ptr.rs`](../../src/machine/core/scope_ptr.rs); the reattach is open-coded at ~18 call
sites (`node_store.rs`, `runtime.rs`, `outcome.rs`, `dispatch/ctx.rs`, …), each carrying a prose SAFETY
note that the producer frame `Rc` pins the value. A node result slot stores the erased value and its
witness frame `Rc` as *separate* fields, so "the witness keeps the value alive" is asserted in comments,
not types. The carriers are invariant — they transitively hold a `Scope`, whose binding table
([`Bindings`](../../src/machine/core/bindings.rs)) is interior-mutable over region-lifetime references —
so [`yoke`](https://docs.rs/yoke), whose safe `get` assumes covariance, cannot express them.

**Acceptance criteria.**

- A top-level `witnessed` module (sibling to `machine` / `scheduler`) owns `Reattachable`, the private
  `retype` primitive, `erase_to_static`, and the `Witnessed<T, W>` carrier; `scheduler` and `machine`
  depend on it.
- `Witnessed<T, W>` bundles the erased value with its liveness witness `W` in one type; the
  witness-pins-the-value relationship is a type invariant, not a comment.
- The module exposes exactly two `unsafe`-bearing accessors, both rank-2 (`for<'b>`) branded: `with`
  (borrow + read) and `map` (consume + transform). No other `unsafe` reattach exists in the
  value-carrier path.
- A scheduler result slot stores a single `Witnessed<Carried, …>`, not a separate `(Erased, frame)`
  pair; `read_result` / `read` / `read_result_with_frame` read it through `with`.
- The ~18 open-coded `reattach_value` / `reattach_slice` / `Erased::reattach` call sites are gone,
  replaced by `Witnessed` accessors or the witness-borrowed `reattach_with` helper.
- The module carries a self-contained Miri tree-borrows slate naming only stand-in families (a covariant
  `&'r u32`, an invariant `Cell<&'r u32>`, a mutable-scope-plus-pool family), plus compile-fail guards
  that neither `with` nor `map` lets a branded reference escape.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` are clean.

**Directions.**

- *Build a bespoke `Witnessed`, not adopt `yoke` — decided.* `Yoke<Scope, _>` requires asserting
  `Yokeable` (covariance) for the invariant `Scope`, and its safe `get` is then a use-after-free anyone
  can call. `Witnessed` makes only a layout-invariance assertion and exposes no unsound safe method.
  Mechanism borrowed from yoke (`map` ≡ `map_project`, `PhantomData<&'b ()>` and all); crate not.
- *Two rank-2 accessors, not one content-free reattach — decided.* `with` (read) and `map`
  (transform/mutate). The naive borrow-bounded / content-free reattach is a Miri-proven use-after-free;
  the `for<'b>` brand is what makes the fabricated lifetime non-escaping.
- *Witness bound via `stable_deref_trait` — decided.* `W: StableDeref` (or a `Witness` marker) makes
  "the witness's pointee does not move while borrowed" a bound rather than prose. `Rc<CallFrame>`
  qualifies. Sole new dependency.
- *Relocate `Reattachable` behind a re-export first — decided.* Move the trait + primitive into
  `witnessed` and leave a `pub use` in `scheduler`, so existing call sites compile unchanged while the
  module lands; migrate call sites incrementally after.
- *`map`'s first consumer — deferred.* The value-carrier migration uses only `with`; a `map`
  consumer (scope mutation inside the brand) has no shipped caller yet. The module ships and tests
  `map` regardless.

## Dependencies

Working notes, the spike crate, and the invariance / accessor-soundness findings live in
`scratch/witnessed-carrier.md`.

**Requires:** none — foundation.

**Unblocks:**
