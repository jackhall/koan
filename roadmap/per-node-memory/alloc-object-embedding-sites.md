# `alloc_object` embedding sites return `Witnessed`

Convert the value-embedding object-construction sites — the ones that wrap a bound value,
a captured scope, or a deep-cloned dep — onto `yoke` / `merge`, so the object family carries
no `Witnessed::new` and the object path reads its reach off the carrier alone.

**Problem.** The region-pure and aggregate object constructions are built inside the witness
closure — a region-pure leaf [`yoke`s](../../src/witnessed.rs), and a list / dict / record
folds its dep [`Sealed`](../../src/witnessed.rs) carriers via `transfer_into`. But the
*value-embedding* sites still pair an already-built `&'a KObject` with an asserted witness
through a structural `Witnessed::new` (or stay on the bare value-copy channel entirely). Each
receives its input as a **bare `Held` arg or a captured `&'a` borrow** — `let_binding` and the
`NEWTYPE` / tagged [`constructors`](../../src/machine/execute/dispatch/constructors.rs) deep-clone
bare `ctx.args`; [`catch`](../../src/builtins/catch.rs)'s `CatchContinue` tags a bare watched
value; [`record_projection`](../../src/builtins/record_projection.rs)'s `FROM` re-tags a bare
record; [`attr`](../../src/builtins/attr.rs)'s `Wrapped` wraps a bare arg; and
[`alloc_function`](../../src/builtins.rs) embeds a captured `scope: &'a Scope` plus a signature
of `&'a KType` refs. A bare arg can neither `yoke` (the `for<'b>` brand rejects the captured
`&'a`) nor `merge` (no carrier is in hand to fold). The
[`resolve_aggregate_bare_name`](../../src/machine/execute/dispatch/literal.rs) **Resolved arm**
sits in the same bind: it wraps a resolved bound value that arrives as a bare `Carried` with no
carrier. So these sites keep the object path *depending on* the read-out
[`reached_frame`](../../src/machine/execute/lift.rs) recovery and the
[`FrameStorage.retained`](../../src/machine/core/arena.rs) accumulator to reconstruct reach, the
two mechanisms the rest of the object family no longer needs.

**Acceptance criteria.**

- Each value-embedding object construction is built inside the witness closure: a single
  embedded dep or bound value (`catch`'s `Tagged`, `attr`'s `Wrapped`, `FROM`'s `Record`, a
  `let`-bound value, a `NEWTYPE` / tagged param) `merge`s the one carrier it embeds, and a
  captured scope (`alloc_function`'s `KFunction`, its body via `alloc_witnessed_embedding`)
  `merge`s its defining frame's `singleton(F)` — never an arbitrary value paired with an
  asserted witness.
- The object family carries no `Witnessed::new`: the
  [`literal.rs`](../../src/machine/execute/dispatch/literal.rs) Resolved arm and every bare
  value-copy object site are converted, so a grep for object-family `Witnessed::new` is empty.
- A lifted object's reached regions are read off its carrier's witness set end-to-end: the
  object path no longer *depends on* the read-out `reached_frame` recovery or the
  `FrameStorage.retained` accumulator. (The two mechanisms are not deleted here — `KType::Module`
  still rides them on the type channel; [`alloc_ktype`](alloc-ktype-witnessed.md) converts that
  last user and deletes both.)
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean. A grep
  confirms zero object-family `Witnessed::new`.

**Directions.**

- *Mechanism for delivering a carrier to a builtin body — open.* A value-embedding site can
  only `merge` if its input arrives as a carrier, but a builtin body reads its inputs as bare
  `Held` args (their carriers discarded at dispatch). Two routes: (a) **thread carriers through
  the args record** — deliver each builtin arg as a `Sealed` so bodies `merge`, relaxing the
  current "nothing new threads through `Carried` / `Held` / `ArgValue`" constraint the shipped
  [construction model](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node)
  holds; or (b) **mint the merge operand from the captured scope
  frame** — a body holding `{F}` (the scope's `region_owner`) `merge`s `singleton(F)`, leaning on
  `F` transitively pinning everything bound into its scope, with no per-arg carrier. Route (b)
  keeps the bare-args plumbing but reintroduces a `singleton(F)`-witnessed structural construction
  unless the embed itself can be expressed as a `merge` against `{F}`. Recommended: settle (a) vs
  (b) per-site by whether the embedded value is *region-local to the body's frame* (→ (b), the
  frame pins it) or *a distinct dep's region* (→ (a), only its own carrier names it).
- *AC2 scope of the merge / relocate-helper `Witnessed::new` — open.* Two non-object-value
  `Witnessed::new` sites remain after the object values convert:
  [`finalize.rs`](../../src/machine/execute/finalize.rs)'s `ContractHomeFamily` operand (the
  declared-return re-stamp's `(home, declared)` pair, consumed inside a `merge`) and
  [`runtime.rs`](../../src/machine/execute/runtime.rs)'s `RegionRefFamily` operand (the `dest`
  region carrier for `transfer_into`). Decide whether AC2's "zero object `Witnessed::new`" covers
  these or exempts them as non-object-value helper carriers feeding a `merge` / relocation.
  ([`finalize.rs`](../../src/machine/execute/finalize.rs)'s bare `finalize_terminal` stays — it
  serves the type channel, errors, and `KType::Module`.)
- *`alloc_function` reuses `alloc_witnessed_embedding` for the body — decided.* The FN body is an
  owned splice-free [`KExpression`](../../src/machine/model/ast.rs), so it yokes via the shipped
  [`alloc_witnessed_embedding`](../../src/machine/core/arena.rs) exactly as `quote` does; the
  captured scope and signature refs fold in via `merge` against the defining frame. The open part
  is only the carrier-delivery mechanism above, not the embedding primitive.

## Dependencies

This item finishes the object-family conversion: the shared dep-result plumbing (the lift hands
each finish its deps' `Sealed` carriers) and the region-pure leaf and aggregate constructions
have shipped (see
[per-node-memory.md § Construction](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node)).
The value-embedding sites that remain need the carrier-delivery mechanism decided above before
they can `merge`.

**Requires:** none — the dep-carrier foundation (`Sealed::duplicate` /
[`Scheduler::dep_carrier`](../../src/scheduler.rs) / the `DepTerminal` carrier) it builds on is
shipped.

**Unblocks:**

- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — completes the `reached_frame` /
  `FrameStorage.retained` deletion by taking the last `KType::Module` user off them; this item
  retires the object path's dependence on the same two mechanisms.
