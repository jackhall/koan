# Per-scope sealed reach-set

Replace the single-frame `reached_frame` / `FrameStorage.retained` reconstruction with a per-scope
reach-set that folds each bound value's full witness and seals when the scope closes.

**Problem.** A region-referencing value bound into a scope has its reach reconstructed at read-out:
[`reached_frame`](../../src/machine/execute/lift.rs) recovers a *single* `Rc<FrameStorage>` from the
value's scope `region_owner`, and the consumer frame accumulates it into the per-frame
[`FrameStorage.retained`](../../src/machine/core/arena.rs). This is a single-frame approximation, and
the [`FrameSet`](../../src/machine/core/arena.rs) witness is already the right shape (the
tree-flattened union the construction path carries on its carriers). `reached_frame` matches only
`KFunction` / `KFuture` / `KType::Module` and returns `Option<Rc<FrameStorage>>`, so a value reaching
*several* regions is mis-recorded — a list / dict / record of closures hits the `_ => None` arm and
retains nothing; a closure capturing several closures records only its own captured-scope frame, the
inner reaches surviving as a chain of per-bind single retains that drops a value one nesting level
down. Binding cannot consume the `FrameSet` that already expresses the union.

**Acceptance criteria.**

- A scope owns a reach-set that is a mutable builder while the scope is active and an immutable
  `FrameSet` once sealed; binding a value folds the *full* `FrameSet` of the bound value's carrier into
  it, so a multi-region value contributes every region it reaches rather than one frame.
- The reach-set omits the scope's own home frame while the scope is resident; the home frame is added
  only when a value is lifted out of that frame. A `let rec` self-binding folds the scope's set into
  itself (a no-op) and never names the home frame, so no reference cycle forms and `FrameStorage`'s
  refcount is not pegged — TCO frame reuse (`try_reset_for_tail`) keeps its three Miri tests.
- A `Scope` is aware of its own close: after close it rejects further binds (rebinds are already
  rejected — see [`bind_value`](../../src/machine/core/scope.rs)), and the close event is where the
  reach-set seals. The reject-after-close assertion never fires across the suite, confirming close is a
  single identifiable event.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Mutable-then-sealed, sealed at scope close — decided.* The set is mutable only while the scope can
  still gain binds; at close (rebinds illegal, no new binds) it seals via a `seal(self) -> Sealed` that
  yields an immutable `FrameSet`. Sealed at *close*, not per-lift: a closure lifted mid-block still
  shares the scope by bare borrow and must see binds added afterward, so it holds the still-growing
  handle until close. Opening a not-yet-sealed set is sound because binds fall *between* scheduler steps
  and an [`open`](../../src/witnessed.rs) runs *within* one — the set never mutates during an access.
- *Omit the home frame; add it on lift — decided.* A resident value must not witness the frame it
  lives in (the region → scope → set → frame cycle). The reach-set names only foreign reach; the home
  frame joins the witness at the lift boundary [`transfer_into`](../../src/witnessed.rs) already marks
  (in-region → held-outside). This is the structural form of the
  [`retain`](../../src/machine/core/arena.rs) self-no-op and subsumes it.
- *Scope close-awareness lands first, as a spike — decided.* Teaching `Scope` to reject post-close
  binds both validates that close is a clean event and is permanent safety; it is the opening move of
  this item, ahead of rewiring reach.
- *Where the set lives — open.* On `Scope` (beside the bindings it describes) vs. keyed off the frame
  `region_owner`. Recommended: on `Scope`, since reach is a property of the scope's bindings and the
  scope is what a closure captures.
- *Substrate vs. workload — open.* Whether the two-phase builder → sealed abstraction is a generic
  `witnessed` primitive or a Koan-specific `FrameSet` wrapper. Recommended: Koan-specific first (it is
  `FrameSet`-shaped), generalized only if a second witness type wants it.

## Dependencies

This is the foundation the construction inversions build on: it stands up the reach-set and the scope
close event but does not itself retire `reached_frame` — the construction sites still on the bare
channel keep reconstructing until they invert.

**Requires:** none — foundation.

**Unblocks:**

- [`alloc_object` embedding sites return `Witnessed`](alloc-object-embedding-sites.md) — its binds fold
  carriers into this reach-set, and `alloc_function`'s closure witnesses the sealed set.
