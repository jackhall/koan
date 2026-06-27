# `alloc_object` embedding sites return `Witnessed`

Convert the value-embedding object-construction sites onto `yoke` / `merge`, and route scope binds
through the [per-scope reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into),
so the object channel names its reach on the carrier and the bindings on the scope set — never
reconstructed.

**Problem.** The region-pure and aggregate object constructions are built inside the witness closure —
a region-pure leaf [`yoke`s](../../src/witnessed.rs), and a list / dict / record folds its dep
[`Sealed`](../../src/witnessed.rs) carriers via `transfer_into`. But the *value-embedding* sites still
pair an already-built `&'a KObject` with an asserted witness through a structural `Witnessed::new` (or
stay on the bare value-copy channel entirely). Each receives its input as a **bare `Held` arg or a
captured `&'a` borrow** — `let_binding` and the `NEWTYPE` / tagged
[`constructors`](../../src/machine/execute/dispatch/constructors.rs) deep-clone bare `ctx.args`;
[`catch`](../../src/builtins/catch.rs)'s `CatchContinue` tags a bare watched value;
[`record_projection`](../../src/builtins/record_projection.rs)'s `FROM` re-tags a bare record;
[`attr`](../../src/builtins/attr.rs)'s `Wrapped` wraps a bare arg; and
[`alloc_function`](../../src/builtins.rs) embeds a captured `scope: &'a Scope` plus a signature of
`&'a KType` refs. A bare arg can neither `yoke` (the `for<'b>` brand rejects the captured `&'a`) nor
`merge` (no carrier is in hand to fold). The
[`resolve_aggregate_bare_name`](../../src/machine/execute/dispatch/literal.rs) **Resolved arm** sits in
the same bind. So these sites keep the object path *depending on* the read-out
[`reached_frame`](../../src/machine/execute/lift.rs) reconstruction (and, for binds, on the per-frame
accumulator) to recover reach — the mechanism the rest of the object family no longer needs.

**Acceptance criteria.**

- Each value-embedding object construction is built inside the witness closure: a single embedded dep
  or bound value (`catch`'s `Tagged`, `attr`'s `Wrapped`, `FROM`'s `Record`, a `NEWTYPE` / tagged
  param) `merge`s the one carrier it embeds, and a captured scope (`alloc_function`'s `KFunction`, its
  body via `alloc_witnessed_embedding`) witnesses the defining scope's sealed
  [reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into) — never an
  arbitrary value paired with an asserted witness.
- A `let`-bound value is a **deposit**, not a construction: `let_binding` folds the bound value's
  carrier `FrameSet` into the [per-scope reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into),
  so a bound list-of-closures contributes every region it reaches. This bind-precise fold **replaces**
  the transitional single-frame relocate-seam fold (the TCO-safe `reached_frame` patch in
  `relocate_dep_into_consumer`) for the object channel: the object value-copy finish stops folding at
  the relocate seam once its bind folds the full carrier, so the two never double-fold the same value.
- The object family carries no `Witnessed::new`: the
  [`literal.rs`](../../src/machine/execute/dispatch/literal.rs) Resolved arm and every bare value-copy
  object site are converted, so a grep for object-family `Witnessed::new` is empty.
- A lifted object's reached regions are read off its carrier's witness set end-to-end, and a bound
  object's off the scope reach-set: the object path no longer *depends on* the `reached_frame`
  reconstruction. (The mechanism is not deleted here — `KType::Module` still rides it on the type
  channel; [`alloc_ktype`](alloc-ktype-witnessed.md) converts that last user and deletes it.)
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Mechanism for delivering a carrier to a builtin body — open.* A value-embedding site can only
  `merge`, and a bind can only fold, if the input arrives as a carrier, but a builtin body reads its
  inputs as bare `Held` args (their carriers discarded at dispatch). Route (a): **thread carriers
  through the args record** — deliver each builtin arg as a `Sealed`, relaxing the current "nothing new
  threads through `Carried` / `Held` / `ArgValue`" constraint. Route (b) — *mint the merge operand from
  the captured scope frame `{F}`* — is **dead for binds and for foreign-reaching embeds**: `{F}` names
  the scope's frame, not the bound value's foreign reach (the multi-region case the
  [reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into) exists to
  capture), so only a value provably region-local to the body's frame may use it. Recommended: route
  (a) wherever the embedded value can reach another region.
- *AC `Witnessed::new` scope — open.* Two non-object-value `Witnessed::new` sites remain after the
  object values convert: [`finalize.rs`](../../src/machine/execute/finalize.rs)'s `ContractHomeFamily`
  operand (the declared-return re-stamp's `(home, declared)` pair, consumed inside a `merge`) and
  [`runtime.rs`](../../src/machine/execute/runtime.rs)'s `RegionRefFamily` operand (the `dest` region
  carrier for `transfer_into`). Decide whether the "zero object `Witnessed::new`" criterion covers
  these or exempts them as non-object-value helper carriers feeding a `merge` / relocation.
  ([`finalize.rs`](../../src/machine/execute/finalize.rs)'s bare `finalize_terminal` stays — it serves
  the type channel, errors, and `KType::Module`.)
- *`alloc_function` reuses `alloc_witnessed_embedding` for the body — decided.* The FN body is an owned
  splice-free [`KExpression`](../../src/machine/model/ast.rs), so it yokes via the shipped
  [`alloc_witnessed_embedding`](../../src/machine/core/arena.rs) exactly as `quote` does; the captured
  scope's reach rides the scope's sealed reach-set, the signature refs fold in via `merge`.
- *Seal the ascribe per-ascription module view — decided.* Folding binds into the scope reach-set must
  also close [`ascribe`](../../src/builtins/ascribe.rs)'s `child_under_module` `new_scope`: it escapes
  via `new_module` like a MODULE body, so once a bound member folds its carrier into that scope's set
  the set must seal. The finalize-time, owner-routed closes shipped with the reach-set (per-call frame /
  `MODULE` / `SIG` / run root) do not cover it — ascribe binds via a synchronous `try_bulk_install_from`
  rather than a dispatched body, so its view stays open. Add a `new_scope.close()` at the ascription
  finish, before `new_module` captures the scope, mirroring the `MODULE` / `SIG` close.

## Dependencies

This finishes the object-family conversion: the shared dep-result plumbing (the lift hands each finish
its deps' `Sealed` carriers) and the region-pure leaf and aggregate constructions have shipped (see
[per-node-memory.md § Construction](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node)).
The value-embedding sites need the carrier-delivery mechanism decided above before they can `merge`.

**Requires:** none — the [per-scope reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into)
foundation it folds binds into has shipped.

**Unblocks:**

- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — completes the `reached_frame` /
  `retained` deletion by taking the last `KType::Module` user off them; this item retires the object
  path's dependence on the same reconstruction.
- [Migrate the consumption reads onto `open`](reads-to-open.md) — the transitional `read` can retire
  only once both construction channels are off the bare read-out, of which this is the object half.
