# Callable copy tuning

The pricing levers the first callable-copy seam
([lazy-closures.md § Lazy close](../../design/lazy-closures.md)) deliberately
leaves unpulled.

**Problem.** The callable copy consolidates an escaping closure at exactly two
surfaces: a top-level `KFunction` crossing the escape seam out of its own
captured region, and an explicit `CLOSE OVER` capture. Both are *decisions*, and
the pricing behind them is provisional.

A **foreign crossing** pins unconditionally, mirroring the substrate rule in
[`copy_or_pin`](../../src/machine/model/values/kobject.rs). A callable that once
downgraded to `Pin` is therefore never re-consolidated by a later escape
crossing — its captured region is foreign to every later host — so frame-death
evacuation is the only backstop for a pin that could have been priced away
earlier.

The comparison a *home* crossing does run reads the chain's summed binding-copy
memos against what the pin would retain, under the tuning constant
`ALPHA_DIVISOR` shared with the substrate chooser. Its own doc comment calls it
"provisional pending measurement": nothing in the tree records a workload on
which a different divisor would have decided differently, so the constant is
unfalsifiable as it stands.

**Acceptance criteria.**

- A ready callable crossing a priced seam consolidates under the same cost
  comparison whether its captured region is the crossing's host or foreign to
  it, and the copy releases only regions the product no longer reaches.
- The chooser's tuning constants are justified by a measured workload
  (smallest `n` that shows the trend), recorded where the constant is defined.

**Directions.**

- *Foreign-crossing pricing — open.* Reuse the home-crossing α against the
  chain regions' allocated totals, or give foreign crossings their own
  constant (the retention profiles differ: a foreign pin is often already
  shared with other holders, so a copy may release nothing).
- *Where the measurement lives — open.* A benchmark under `observe/` recording
  the crossover, or a test asserting the decision flips at a stated size. The
  first documents the constant, the second defends it against drift.

## Dependencies

Region evacuation owns the frame-death backstop for pins this pricing never
reaches. Three capability gaps are carved out of this item rather than tuned
here: [Module scope consolidation](module-scope-consolidation.md) and
[Callable cells in copied containers](callable-cells-in-copied-containers.md)
are environments the engine cannot rebuild at all, and
[A flattened dispatch registration pins its defining frame](flattened-registration-pins-its-frame.md)
is a decline taken at `CLOSE`'s flatten, before any crossing.

**Requires:** none — the first callable-copy seam it tunes is shipped.

**Unblocks:** none.
