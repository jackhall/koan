# Destination-homed construction

A node may be marked with a **destination**: a region other than its own
incarnation's, in which its result value is constructed. The value is *born
at the destination* — it never exists in the producer's region, so there is
nothing to move, nothing to invalidate, and no ownership-transfer operation
for the programmer to reason about. This is not move semantics; it is the
idea [memory-model.md](memory-model.md) already applies at the
implementation layer ("`Scope` itself is *born* at its destination"; the
delivery walk adopting terminals into each edge's destination region)
hoisted from delivery time to construction time. Where the delivery adopt
pays a structural copy for producer-born data, a destination-homed
constructor pays nothing: the data was never anywhere else.

## Propagation along result positions

The mark flows along **result positions, not dataflow**:

- the marked node's result expression;
- block tails;
- the result expressions of `IF` / `MATCH` / `TRY` arms (each arm carries
  the mark independently);
- across a call boundary into the callee's own result positions, riding a
  per-call channel of the same shape as the kept-first return contract
  ([tail-call-optimization.md](tail-call-optimization.md) — though not that
  channel itself; § TCO constraints).

A constructor node (record, list, closure, arithmetic result) in marked
position builds at the destination. A node that merely *selects* existing
data contributes a reference governed by the crossing rule below — nothing
to redirect. Mixed values fall out per part: a pair constructor builds at
the destination, its producer-born field builds there too, its
reference-into-destination-owned-data field crosses free.

**What the rule refuses is load-bearing.** A value bound to a local and
delivered later never receives the mark: it is built in the producer's own
region and crossed at delivery by the ordinary adopt, copying if
producer-born. Only expressions whose value *definitely* reaches the
destination build there, so speculative homing — and the leak of rejected
candidates into a long-lived destination region — is unrepresentable rather
than merely discouraged. The teachable line: *what you write in the
delivering position goes straight to the destination; what you stash first
lives with you.*

## The crossing rule and its cost tiers

Destination-homed construction presupposes the anti-ring crossing rule: a
delivery may not leave the destination holding any region whose ownership
chain passes back through the destination — holds upward and sideways are
fine; loop-back holds are the pin ring, and the parts that would mint one
copy instead. A value built at the destination must satisfy the same rule at
construction: its parts either build at the destination themselves
(propagation handles this) or cross as legal references. The primitive is
the rule's zero-copy cost tier, not a replacement for it:

| Tier | When | Cost per delivery |
| --- | --- | --- |
| Static copy | creation-time reach shows no destination-side storage reachable | unconditional copy, no per-crossing test |
| General crossing rule | default | copy producer-born parts; references cross free |
| Destination-homed construction | destination proven at construction time (result-position mark) | zero — built in place |

## Surface

Follows the `CLOSE` grain ([lazy-closures.md](lazy-closures.md)): koan's
surface control names **values and crossings, never regions**. Region
identity stays invisible to the programmer. The primitive is therefore
*inferred* — in a delivering position, the position itself is the mark; no
new syntax. `CLOSE OVER` remains the surface for the opposite direction
(consolidate by copy). The design must not require an explicit surface
form.

## Clients

Nothing in the primitive is delivery-kind-specific; it is specified so each
client adopts it without rework, in this order:

1. **Yield deliveries** — the first client
   ([roadmap/foundation/yielding-iterators.md](../roadmap/foundation/yielding-iterators.md)).
   A demand-driven generator gets the primitive's preconditions for free:
   the destination is *known* before construction (the generator resumes
   only after a consumer mints the demand edge), *stable* for the window (a
   demanding consumer is parked, so its region cannot turn over
   mid-construction), *singular* (one demand edge), and *provably existing*
   (the consumer allocated the demand). The copy the crossing rule
   prescribes for producer-born element parts becomes a build-in-place. A
   filtering generator's rejected candidates are built in the generator's
   own per-iteration region and die at the tail call — result-position
   propagation cannot reach them. One residual: pre-built producer-owned
   data (a generator that materializes a table eagerly, then yields rows)
   still copies each row out; the rows predate every demand, and the copy
   is the visible price of the eager shape.
2. **Returns and bindings.** `Scope::adopt_for_binding` relocates a
   callee's result into the caller's frame region by copy at every `let` /
   user-fn arg / `USING`. Result-position threading of the caller's
   destination makes those results born in the caller's frame — the
   copy-at-return removed generally, not just at yields.
3. **Loop-carried arguments.** The one structural copy per tail hop
   ([tail-call-optimization.md § Loop-carried values](tail-call-optimization.md))
   is an argument-position destination: the fresh cart's region. The
   hardest client; pursued only if the TCO constraints below resolve
   cleanly.

## TCO constraints

The tail-call design ([tail-call-optimization.md](tail-call-optimization.md))
constrains the primitive in six ways. Two are core and unresolved; two scope
which clients the primitive can reach; two have resolution directions the
design adopts.

### Core, unresolved

- **Step-time foreign allocation crosses two guarded lines.** The
  run-then-apply ordering (Lemma 1) and the library boundary
  ([scheduler-library.md](scheduler-library.md)) both assume a step
  allocates only into its own incarnation's region under its own brand.
  Building into a foreign region mid-step needs an allocating capability
  and reach-table mint on a region the library owns on behalf of a
  *different* node — a new boundary API — and opens the aliasing window of
  reading the producer's region while writing the destination's, the shape
  the no-reserve-frame decision exists to avoid. Every client, including
  the yield client, needs this solved. The fold/brand machinery already
  composes source regions by union with a destination brand in scope
  ([memory-model.md](memory-model.md)); the open question is what grants a
  *step* that brand for a region it does not own, and the grant's lifetime
  discipline.
- **The kept-first precedent does not carry the mark.** The kept-first
  return contract survives every tail hop precisely because it is pure
  `Copy` data — it references no region and pins nothing. A destination
  mark is region-ful. Threading it through that channel would import a
  retention question into a channel whose soundness argument is that it
  touches no region. Either the mark rides as a region-free name — slot
  plus edge, resolved to a region only at construction — or it needs a
  channel of its own with an explicit retention story.

### Client-scoping

- **Destination multiplicity, decided late.** A loop terminal can fan out:
  bare-name forwards splice parked edges onto the loop slot mid-loop
  ([tail-call-optimization.md § Interaction with bare-name forwarding](tail-call-optimization.md)).
  When the final iteration's constructor runs, the destination set is
  plural and may still be growing. Delivery-time adoption handles this
  naturally; construction-time homing must decline the mark under possible
  fan-out or pick a primary destination and copy to the rest.
- **Dynamic finality thins the marked set inside loops.** Which iteration
  produces a loop's result is a runtime branch; an accumulator is
  constructed iterations before its deliver-hood is known, so
  result-position marks reach only the final hop's tail expression —
  usually a pass-through, which already rides free. The copies that recur
  (loop-carried argument adoption) sit in argument position. Inside a TCO
  loop the primitive's conservative propagation has little to optimize; the
  loop-carried-argument client needs its own destination story.

### Resolved by the design

- **Regions are not stable names; slots are.** A destination held as a
  region handle goes stale across the consumer's own tail hops; held as a
  pin, it defeats the consumer's O(1) turnover. The mark therefore names
  **(slot, edge)** and resolves to a region at the latest moment — for
  yields, at demand time, which is both late enough and early enough.
- **The destination may not exist when marked expressions run.** Region
  mint is lazy — allocation-light frames own no region, and a tail call's
  fresh cart is minted during the decide, after operand evaluation. Late
  resolution plus an on-demand mint at the first destination-homed
  allocation covers this, accepting that a mark can force a mint a
  region-free frame would otherwise skip.

## Open work

- [roadmap/foundation/destination-homed-construction.md](../roadmap/foundation/destination-homed-construction.md)
  — the primitive itself: the foreign-region grant mechanism, the mark's
  carrier, the fan-out posture, and the client sequencing.
- [roadmap/foundation/yielding-iterators.md](../roadmap/foundation/yielding-iterators.md)
  — the first client's substrate: the delivering park, demand edges, and
  the iterator surface.
