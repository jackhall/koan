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

**Acceptance criteria.**

- Both binding tables store one `Sealed`-shaped witnessed carrier that never
  separates a value from its proven reach: `data: name → (BindingIndex,
  Sealed<CarriedFamily, …>)` and `functions: key → Vec<(BindingIndex,
  Sealed<KFunctionFamily, …>)>`. Functions travel as a `KFunctionFamily` — the
  witnessed library is generic over `Reattachable` families, so no `Carried`
  variant is added.
- One value family flows through three carrier states connected by transform
  verbs, never by wrapping: `Delivered` (owning members inline, in transit —
  scheduler slots, cross-frame escapes, `ReturnContract`), `Sealed` (weak,
  arena-hosted description, at rest in table entries and node slots), and
  `Opened<'b>` (weak, borrowing at a step lifetime `'b`). The verbs are
  `Sealed::open_at`, `Opened::reseal`, the `Sealed → Delivered` lift (weak members
  upgraded to an owned inline set under still-live ambient coverage), and the
  `Delivered → Sealed` adopt (mint into the destination arena, retain the owned
  set, drop to weak).
- The runtime-audited re-box is deleted: the fold door's witnessed product seals
  straight into the table (`rebuild_delivered_substrate`'s `alloc_object_checked`
  plus deep clone no longer runs).
- Home is an ordinary `Weak` reach member; `StoredReach`'s home omission, the
  borrows-home bit, and their mint/omit plumbing are deleted. The home/foreign
  distinction is one region-identity rule at the owned-upgrade boundary: never pin
  a region into itself.
- Binding entries own nothing — they are `Copy` and `Drop`-free. The scope owns one
  deduped `RefCell<PinBundle<F>>` union bundle that each bind door unions its lifted
  owned pins into (applying the self-pin rule), so frame death drops one union, not
  N per-entry bundles, and a carrier read hands out a claim with no `FramePins`
  clone.
- `Reached`, `StoredReach`, `ValueHit`'s `{obj, stored, pins}` triple, and
  `MemberResolution::Value`'s triple are retired in favor of the carrier; ATTR
  replay-token semantics are preserved because the read replays the stored claim.
- Dispatch resolves on `Opened<'step, KFunctionFamily>` carried by
  `Resolved<'step>` across argument evaluation; the escape into the call chain
  re-seals into a `ReturnContract` holding a `Delivered`.
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
- *Liveness home — decided.* One deduped scope-owned union bundle;
  `PinBundle::union` already dedupes by region identity with outer-chain
  subsumption (an antichain of the deepest owners), so no new dedup machinery is
  needed. The region retention list keeps its existing non-binding uses.
- *Phasing — decided.* (1) library — `Opened<'b>` plus the four transform verbs,
  home-as-member, `KFunctionFamily`, `Delivered` reshaped to own its member set
  inline; (2) the `data` table; (3) the `functions` table with dispatch on
  `Opened<'step>` and `ReturnContract` holding `Delivered`; (4) ascription — drop
  the `ascription` feature flag and migrate the feature onto the carrier surface.
- *Ascription flag — temporary.* Module ascription (`:|` / `:!`) is gated off behind
  a `ascription` cargo feature so phases 1–3 need not carry the heaviest consumer of
  the machinery they retire — it drives `store_transparent_view`,
  `child_module_reach`, `Reached`'s reader impl, and the `sig_subtype` /
  `SigSubtypeFailure` surface. Phase 4 deletes the feature and every gate, migrates
  the two ascription store doors off `Reached::mint(value, claim, pins)` onto the
  sealed entry plus the scope union bundle, and restores `tools/verify.sh`'s tutorial
  snippet step. The flag is not an end state: this item is not done while it is off.
- *Step-as-fold — deferred.* Placing the whole step body under one rank-2 brand to
  subsume the per-site open/reseal doors is a later simplification, not this item.

## Dependencies

**Requires:**

- [Residence-audit retirement](residence-audit-retirement.md) — the fold-brand
  construction doors whose confined witnessed product these carriers seal into.

**Unblocks:**

- [Drop-free region death](drop-free-region-death.md) — removes the `data` table's
  typed-and-droppy residue, so that item can migrate the entries into the untyped
  bump arena.
