# Carrier-delivered object embeds and the relocate-seam-fold retirement

Deliver each builtin arg's `Sealed` carrier into the builtin body so the remaining value-embedding
object sites can `merge`, fold a `let`-bound value's full carrier into the
[per-scope reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into), and
retire the transitional single-frame relocate-seam fold — converting the user-fn arg-bind path that
shares it.

**Problem.** The carrier-self-building object constructions are witnessed (the aggregates, the
[`constructors`](../../src/machine/execute/dispatch/constructors.rs) and
[`catch`](../../src/builtins/catch.rs) folding their dep carriers, the FN-def
[`finalize`](../../src/builtins/fn_def/finalize.rs) `yoke`ing its co-located `KFunction`). The
remaining value-embedding sites cannot follow them, because each receives its input as a **bare `Held`
arg or a captured `&'a` borrow**, not a carrier: [`let_binding`](../../src/builtins/let_binding.rs)
deep-clones a bare bound value, [`attr`](../../src/builtins/attr.rs)'s `Wrapped` wraps a bare arg, and
[`record_projection`](../../src/builtins/record_projection.rs)'s `FROM` re-tags a bare record. A bare
arg can neither `yoke` (the `for<'b>` brand rejects a captured `&'a`) nor `merge` (no carrier is in
hand to fold). The [`literal.rs`](../../src/machine/execute/dispatch/literal.rs) **Resolved arm** sits
in the same bind and is the one remaining object-family `Witnessed::new`. So these sites keep the
object path *depending on* the read-out [`reached_frame`](../../src/machine/execute/lift.rs)
reconstruction (and, for binds, on the single-frame relocate-seam fold) to recover reach — the
mechanism the rest of the object family no longer needs.

The relocate-seam fold cannot simply be deleted once these convert: the
[`reached_frame`](../../src/machine/execute/lift.rs) fold in `relocate_dep_into_consumer` **also serves
user-fn object args** — a closure passed to a user-defined function flows through the shared eager-subs
[`short_circuit`](../../src/machine/execute/outcome.rs), a *non*-construction path with no carrier to
fold. The seam fold correctly folds nothing for a region-pure value (pegging a producer frame there is
the TCO hazard the design notes), so it cannot be replaced by a carrier-fold *at the seam*. Retiring it
requires converting the user-fn arg-bind path to fold its dep carriers into the per-call frame's scope
reach-set — a precondition for
[`alloc_ktype`](alloc-ktype-witnessed.md)'s `reached_frame` deletion that the chain did not separately
call out.

**Acceptance criteria.**

- Each builtin arg arrives at the body as a `Sealed<CarriedFamily, FrameSet>` carrier: a terminals-aware
  eager-subs finish threads it to `run_action_builtin` / `BodyCtx`, relaxing the current "nothing new
  threads through `Carried` / `Held` / `ArgValue`" constraint, so a value-embedding body has a carrier
  to fold.
- Each remaining value-embedding object construction is built inside the witness closure: `attr`'s
  `Wrapped`, `FROM`'s `Record`, and the [`literal.rs`](../../src/machine/execute/dispatch/literal.rs)
  Resolved arm `merge` the one carrier they embed — never an arbitrary value paired with an asserted
  `Witnessed::new` — so a grep for object-family `Witnessed::new` is empty.
- A `let`-bound value is a **deposit**: [`let_binding`](../../src/builtins/let_binding.rs) folds the
  bound value's carrier `FrameSet` into the
  [per-scope reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into), so a
  bound list-of-closures contributes every region it reaches. This bind-precise fold **replaces** the
  transitional single-frame relocate-seam fold for the object channel.
- [`ascribe`](../../src/builtins/ascribe.rs)'s `child_under_module` `new_scope` seals: a
  `new_scope.close()` fires at the ascription finish before `new_module` captures the scope, mirroring
  the `MODULE` / `SIG` close, so a member folded into that scope's set is sealed in.
- The user-fn object-arg bind path folds its dep carriers into the per-call frame's scope reach-set, so
  no value remains on the single-frame relocate-seam fold: with the object construction, the binds, and
  the user-fn args all carrying their reach, `relocate_dep_into_consumer`'s `reached_frame` fold serves
  only the type channel's `KType::Module` — the remaining user [`alloc_ktype`](alloc-ktype-witnessed.md)
  deletes.
- A lifted object's reached regions are read off its carrier's witness set end-to-end, and a bound
  object's off the scope reach-set: the object path no longer *depends on* the `reached_frame`
  reconstruction.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Thread carriers through the args record — decided.* A value-embedding site can only `merge`, and a
  bind can only fold, if its input arrives as a carrier, but a builtin body reads its inputs as bare
  `Held` args (their carriers discarded at dispatch). Deliver each builtin arg as a `Sealed`, relaxing
  the "nothing new threads through `Carried` / `Held` / `ArgValue`" constraint. The alternative — mint
  the merge operand from the captured scope frame `{F}` — is dead for binds and foreign-reaching embeds:
  `{F}` names the scope's frame, not the bound value's foreign reach (the multi-region case the
  [reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into) exists to
  capture), so only a value provably region-local to the body's frame could use it.
- *The literal Resolved arm needs a new mechanism — open.* The
  [`literal.rs`](../../src/machine/execute/dispatch/literal.rs) Resolved arm is the genuine "asserted
  co-location" case: the resolved value is a pre-existing lexical-ancestor value, **not** built from the
  classify frame's region, and a resolved closure keeps ancestor borrows that `yoke`'s `for<'b>` brand
  rejects — so neither `yoke` nor `merge` expresses it. Options: make Resolved names into deps so they
  carry a real carrier; or add a witness-bounded reattach constructor for a pre-existing co-located
  value. Recommended: prefer the deps route, so the arm folds a real carrier like every other site.
- *User-fn arg conversion is a precondition for `reached_frame` deletion — decided.* The relocate-seam
  `reached_frame` fold also serves user-fn object args (the shared eager-subs `short_circuit`), so it
  cannot be replaced by a carrier-fold at the seam (that would peg producer frames for region-pure
  values — the TCO hazard). Converting the user-fn arg-bind path to fold dep carriers into the per-call
  frame's scope reach-set lands here, so [`alloc_ktype`](alloc-ktype-witnessed.md) can delete
  `reached_frame` with only the type channel's `KType::Module` left on it.

## Dependencies

The carrier-self-building object constructions (the aggregates, the newtype / tagged-union
constructors, `catch`, FN def) are witnessed; this item converts the value-embedding sites that take
bare args and retires the transitional relocate-seam fold for the object and user-fn-arg paths.

The [per-scope reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into) it
folds binds into and the dep-carrier delivery to construction finishes have shipped.

**Requires:**

- [Carrier-self-building object constructions return `Witnessed`](alloc-object-embedding-sites.md) — the
  bare-arg value-embedding sites and the seam-fold retirement build on these carrier-self-building
  conversions and their witnessed-construction machinery.

**Unblocks:**

- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — converting the user-fn arg-bind path
  off the single-frame relocate-seam fold leaves only `KType::Module` on `reached_frame`, the precondition
  for its deletion.
- [Migrate the consumption reads onto `open`](reads-to-open.md) — the transitional `read` can retire
  only once both construction channels are off the bare read-out, of which this is the object half.
