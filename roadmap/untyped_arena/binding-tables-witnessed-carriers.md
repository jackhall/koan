# Binding tables as witnessed carriers

The two binding tables store the witnessed carrier itself, so a value is never
separated from the reach that proves it.

**Problem.** The `data` binding table stores a naked `&'a KObject` beside a
separately-stored reach (`Reached { value, claim: StoredReach, pins: FramePins }`),
and the `functions` table stores `(&'a KFunction, BindingIndex)` with no reach at
all. Because value and proven reach are split, every reaching move-in must convert
the fold door's confined witnessed carrier back into a bare ref through the
runtime-audited re-box —
[`rebuild_delivered_substrate`](../../src/machine/core/scope/reach.rs)'s
`alloc_object_checked` plus a deep clone. The `functions` table is worse: the
`try_apply` mirror write keeps no reach, and dispatch's picked `&KFunction` escapes
through `Resolved` into `ReturnContract` with zero pins. Each binding entry also
owns its own `FramePins` bundle, so every carrier read clones the bundle and frame
death drops N per-entry bundles; the `data` table's entries are the "typed and
droppy" residue that [drop-free region death](drop-free-region-death.md) has to
carve around. `StoredReach` further omits its home region and re-materializes it
through a "borrows-home" bit to dodge the `frame → region → scope → bindings →
frame` `Rc` cycle.

Koan also owns pins directly. It assembles the step's coverage
([`run_loop.rs`](../../src/machine/execute/run_loop.rs)), threads owned `FramePins`
through [`StepCarried`](../../src/machine/execute/step_carried.rs) and the seam
helpers, and picks a relocation's source claim by hand
([`lift::seam_source_pins`](../../src/machine/execute/lift.rs)) — a bundle assembled
before the copy exists, whose three-way choice is a use-after-free if it is wrong.
Because Koan can hold and drop a bundle, the pinning invariant is a rule it is asked
to honor rather than one it lacks the vocabulary to break. `Scope::covers_region_ambiently`
compounds this: it narrows the *stored description*, not just the owned bundle, so a
description states a value's reach relative to one destination's ambient coverage
rather than the value's own reach — and every consumer, including
`ResidenceEvidence::covers_region`'s third arm, has to re-apply the same predicate to
recover the answer.

**Acceptance criteria.**

- Both binding tables store one `Sealed`-shaped witnessed carrier that never
  separates a value from its proven reach: `data: name → (BindingIndex,
  Sealed<CarriedFamily, …>)` and `functions: key → Vec<(BindingIndex,
  Sealed<KFunctionFamily, …>)>`. Functions travel as a `KFunctionFamily` — the
  witnessed library is generic over `Reattachable` families, so no `Carried`
  variant is added.
- One value family flows through three carrier states connected by transform
  verbs, never by wrapping: `Delivered` (owning members inline, in transit —
  nameable but pin-opaque, held by retention holds and the container verbs), `Sealed`
  (weak, arena-hosted description, at rest in table entries and node slots), and
  `Opened<'b>` (weak, borrowing at a step lifetime `'b`). The verbs are
  `Sealed::open_at`, `Opened::reseal`, the `Sealed → Delivered` lift (weak members
  upgraded to an owned inline set under still-live ambient coverage), and the
  `Delivered → Sealed` adopt (mint into the destination arena, retain the owned
  set, drop to weak).
- The runtime-audited re-box is deleted: the fold door's witnessed product seals
  straight into the table (`rebuild_delivered_substrate`'s `alloc_object_checked`
  plus deep clone no longer runs).
- Home is an ordinary `Weak` reach member; `StoredReach`'s home omission and its
  mint/omit plumbing are deleted. The home/foreign distinction is one
  region-identity rule at the owned-upgrade boundary: never pin a region into
  itself. (The borrows-home bit outlives this item: a fold-born substrate carrier
  has no description to hold home as a member, so retiring the bit is
  [home lives in the reach description](home-lives-in-the-reach-description.md).)
- Binding entries own nothing — they are `Copy` and `Drop`-free. The **region** owns
  one deduped `PinBundle<F>` union bundle that each bind door folds its lifted owned
  pins into (applying the self-pin rule), so frame death drops one union, not N
  per-entry bundles, and a carrier read hands out a claim with no `FramePins` clone.
  `Scope` holds no pin state.
- Koan has no pin vocabulary. `PinBundle` is crate-private to `workgraph`, and
  every owned pin crosses the boundary as an opaque `StepCoverage` — a holder Koan
  may hold, thread and drop, but whose set arithmetic (union, subsumption,
  narrowing, member removal) has no public surface. Every pull, adopt, relocate and
  seal is a container verb on the holder that owns the pins (a node slot, a region,
  a step). Koan still *names* `Delivered`, because an in-transit envelope has to
  appear in its own type positions (a parked node slot, a dep terminal, a finish
  callback's result) and a step's dep slice must own its envelopes while the
  scheduler is mutably borrowed; what Koan cannot do is see inside one or build one
  from loose pins. `Scope::envelope_reach_of` / `copied_reach_of` /
  `pinned_reach_of`, the `carrier_witness.rs` source-claim helpers, and
  `lift::seam_source_pins` / `copied_seam_source_pins` are all deleted, and no koan
  source file names an owned pin bundle. (The `with_home_region` /
  `PinBundle::any_member_region` probes outlive this item: the relocation
  predicates still recover home through them, so their deletion is
  [home lives in the reach description](home-lives-in-the-reach-description.md).)
- A relocation verb derives its source claim from a workload predicate run on the
  built product — `still_borrows(product, source_region)` — rather than accepting a
  bundle or verdict computed before the fold. A `false` answer drops the source
  region from the composed bundle; a `true` answer keeps it.
- Mints apply only the self rule and subsumption: `Scope::covers_region_ambiently`
  and every `omit` parameter are deleted, so a stored description names a value's
  whole reach and a `Sealed` carrier lifts with no policy threaded in.
  `ResidenceEvidence::covers_region`'s `ambient` arm is gone;
  `Scope::chain_reaches_region` survives alone for `runtime/submit.rs`'s cart check.
- `Reached`, `StoredReach`, `ValueHit`'s `{obj, stored, pins}` triple, and
  `MemberResolution::Value`'s triple are retired in favor of the carrier; ATTR
  replay-token semantics are preserved because the read replays the stored claim.
- Dispatch resolves on `Opened<'step, KFunctionFamily>` carried by
  `Resolved<'step>` across argument evaluation; the escape into the call chain
  re-seals into a `ReturnContract` holding a `Sealed`, re-opened per step under the
  step's coverage.
- The bare `NameLookup<&'a KObject>` read shape is retired from production:
  [`builtins/attr.rs`](../../src/builtins/attr.rs) drops its bare arm,
  `sig_schema`'s `iter_data` walk opens under the frame pin, `lookup_kfunction` is
  deleted, and any surviving bare read is `#[cfg(test)]` only.
- [design/witness-hosting.md](../../design/witness-hosting.md)'s retention and
  composition sections are rewritten from the bit + host-materialization scheme to
  the three-carrier / home-as-member end state, and the "binding tables remain
  typed and droppy" prose in [drop-free region death](drop-free-region-death.md)
  and [design/value-substrates.md](../../design/value-substrates.md) is reconciled
  with the `data` table becoming `Drop`-free.
- The temporary `ascription` cargo feature is deleted along with every `cfg` gate it
  guards, module ascription (`:|` / `:!`) runs on the carrier surface with its store
  doors sealing into the table, and `tools/verify.sh` runs the tutorial snippets
  again.
- The Miri audit slate is green.

**Directions.**

- *Family reuse — decided.* Objects reuse the existing `CarriedFamily`; functions
  get a new `KFunctionFamily` rather than a `Carried` enum variant, because the
  library is already generic over `Reattachable` families.
- *Liveness home — decided.* One deduped **region**-owned union bundle.
  `Region::retained_reach` already holds the pins; collapsing its `Vec<PinBundle>`
  to a single bundle folded through `PinBundle::insert` (which dedupes by region
  identity with outer-chain subsumption) *is* the union bundle, so this is a
  deletion rather than new machinery. Region-owned rather than scope-owned because
  bind-once entries make the two death schedules identical, and a region is a
  library type — which is what keeps the union out of Koan's hands.
- *Embedder boundary — decided.* `PinBundle` goes crate-private and Koan reaches
  the transform verbs through container verbs on the pin-owning holder, with owned
  pins crossing the boundary only as the opaque `StepCoverage`. The container
  supplies the home owner each verb needs, so Koan never recovers a producer region
  from a member set, and the `with_home_region` / `PinBundle::any_member_region`
  probes are deleted rather than kept — the deletion itself carried by
  [home lives in the reach description](home-lives-in-the-reach-description.md),
  since the relocation predicates still read them. `Delivered` stays nameable: Koan's own type
  positions (node slots, dep terminals, finish callbacks) need it, and a step's dep
  slice must own its envelopes across a step that mutably borrows the scheduler, so
  an `Opened<'b>` borrowed from the retention hold does not type. Nameable is not
  the same as transparent — the envelope's pins have no public accessor, and the
  invariant Koan lacks the vocabulary to break is the one that matters.
- *Source claim — decided.* Derived by the library from a workload predicate run on
  the built product, not supplied by Koan as a bundle or a mode enum. A checked
  property of the bytes that exist beats a promise made before they are written, and
  it leaves the workload a memory-versus-CPU lever whose conservative answer costs
  retention rather than soundness.
- *Omission — decided.* Deleted, in favour of exact descriptions: narrowing the
  description makes a `Sealed` carrier describe a pairing rather than a value, which
  is exactly what would force a policy callback into every library-side lift. It also
  buys no retention — the chain that made a region ancestral outlives the destination
  anyway. Its cycle-avoidance job does **not** fall to the self rule: omission was
  incidentally masking a two-region ring (a per-call region owning the run-root host
  while the run-root region owns the per-call host) that the self rule does not see,
  because neither owner is the other's destination and a per-call frame's `outer` is
  `None`. That job belongs to the **eternal rule** —
  `PinsRegion::needs_no_pin`, applied at `Region::retain_reach`, which drops
  run-root-tier members from any region-lifetime retention
  ([design/witness-hosting.md § The eternal rule](../../design/witness-hosting.md#the-eternal-rule)).
  Re-adding omission as a bundle-only shrink, leaving descriptions exact, is a later
  optimization if refcount traffic warrants it.
- *Phasing — decided.* (1) library — `Opened<'b>` plus the four transform verbs,
  home-as-member, `KFunctionFamily`, `Delivered` reshaped to own its member set
  inline; (2a) foundation — omission deleted and the region union bundle collapsed,
  both ahead of the table reshape so the bind doors are written once against the
  final mint contract; (2b) the `data` table; (2c) boundary cutover — the retention
  predicate, then the container-verb surface and the crate-private
  `PinBundle`; (3) the `functions` table with
  dispatch on `Opened<'step>` and `ReturnContract` holding a `Sealed`; (4)
  ascription — drop the `ascription` feature flag and migrate the feature onto the
  carrier surface.
- *Step-as-fold — deferred.* Placing the whole step body under one rank-2 brand to
  subsume the per-site open/reseal doors is a later simplification, not this item.

## Dependencies

**Requires:**

- [Residence-audit retirement](residence-audit-retirement.md) — the fold-brand
  construction doors whose confined witnessed product these carriers seal into.

**Unblocks:**

- [Substrate birth mints its home](home-lives-in-the-reach-description.md) — establishes
  home as an ordinary reach member, which that item extends to birth sites.
- [Region-hosted operator groups](region-hosted-operator-groups.md) — establishes
  the sealed-carrier entry shape the third binding table would join.
- [Drop-free region death](drop-free-region-death.md) — removes the `data` table's
  typed-and-droppy residue, so that item can migrate the entries into the untyped
  bump arena.
