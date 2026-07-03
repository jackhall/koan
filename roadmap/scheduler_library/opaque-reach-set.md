# The opaque reach set

**Problem.** Guarantee 2 of
[design/scheduler-library.md](../../design/scheduler-library.md) — a reach set
always names every region a value's borrows can reach — holds by convention,
not by type. `FrameSet` ([arena.rs](../../src/machine/core/arena.rs):489) is
Koan-side with open construction (`empty` / `singleton`) and Koan-side member
folds (`fold_foreign` / `fold_foreign_omitting`), so any caller can assemble a
set by hand; nothing ties a set's members to the regions actually reached. And
because `MergeWitness::merge`
([witnessed.rs](../../workgraph/src/witnessed.rs):266) returns `Option<Self>` —
single-`Rc` witnesses have no representable union — every set-side union pays a
hand-asserted totality `expect`: eleven production sites across
`builtins/catch.rs`, `dispatch/literal.rs`, `dispatch/constructors.rs`,
`execute/finalize.rs`, `run_loop.rs`, and `dispatch/single_poll.rs` repeat
`"a FrameSet set witness always represents the union"`, plus `reseal_under`'s
own inside the library.

**Acceptance criteria.**

- The reach set is an opaque library type (working name `RegionSet`): outside
  the library it is mintable only from region handles and carriers, and no
  Koan code constructs, iterates, or edits its membership directly.
  `FrameSet`'s carrier-witness role is carried by this type.
- Union and transfer over set witnesses are infallible in the signature:
  `Witnessed::merge`, `Sealed::transfer_into`, and `Witnessed::reseal_under`
  on the set-witness path return the carrier with no `Option`, and every
  `expect("… always represents the union")` production site is deleted.
- Per-scope reach policy (which regions a fold omits: home-pinned, lexical
  ancestors) is passed to library fold operations as a predicate; policy code
  composes the set through the library surface only.
- Reach-set contents and scheduler decisions are unchanged — existing tests
  and the Miri audit slate green.

**Directions.**

- *Where subsumption lives — open.* `FrameSet`'s outer-chain subsumption calls
  `FrameStorage::pins_region` — a Koan-shaped member walk. (a) A generic
  library `RegionSet<S>` over a member trait exposing the pins-region hook
  (mechanism library-owned, member semantics workload-supplied); (b) a
  library-owned wrapper that seals minting while the member type and
  subsumption walk stay in `arena.rs`. Recommended: (a) — (b) restricts the
  constructor without making by-hand assembly inexpressible.
- *How merge becomes total — open.* (a) Split the trait: total merge on set
  witnesses only, with single-`Rc` witnesses lifting via `into_set` before
  joining a union; (b) keep the fallible trait for generic code and give the
  carrier combinators total set-specific overloads. Recommended: (a) — one
  currency at the union sites, and the `Option` leaves the API instead of
  being satisfied internally.
- *Read accessors — decided.* Opacity constrains construction, not
  observation: `sole()` / `is_empty()`-style reads stay as library accessors
  (the consumer-pull lift's singleton recovery keeps working).

## Dependencies

[Own the chain-reaches-region predicate](../refactor/fold-chain-reaches-region-predicate.md)
consolidates the same fold loops this item moves behind the library surface;
no hard edge either way, but landing it first shrinks this item's
policy-predicate migration.

**Requires:** none — the boundary move is ready as-is.

**Unblocks:**

- [The step construction context](step-construction-context.md) —
  `alloc_with` folds dep reaches through the library-minted set, infallibly.
