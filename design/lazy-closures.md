# Lazy closures

How koan severs a value's captured environment from the regions it was built
in: capture is by reference, retention is by pin, and severance is always a
priced *copy* — explicit at the `CLOSE OVER` surface, implicit at the escape
seam once a fat pin is forming. "Lazy" names the placement of the cost: a
closure's definition is O(1) always; the transitive copy of its environment
happens at the latest moment reach evidence justifies it, or never.

## Capture pins; the region is the granule

A function value captures its definition scope as one reference
([`KFunction.captured`](../src/machine/core/kfunction.rs)); an escaping value
pins the region owning that scope, and pins chain — through
`FrameStorage.outer` and through the pinned region's own binding entries'
pins ([memory-model.md](memory-model.md)). Retention is per *region*, never
per value: pinning "just one binding" keeps its whole frame region. So
minimal-capture-at-definition cannot fix fat-frame retention — only copying
captures out can, and that copy is exactly what the escape seam already
prices for plain data ([value-substrates.md](value-substrates.md)). What the
seam cannot do alone is copy through a callable: functions and modules are
borrow leaves of the copy verb.

## CLOSE OVER: explicit severance

```
CLOSE OVER (capture1 capture2 (HELPER _) ...) (
  <statements>
  <tail statement>
)
```

The form is a builtin like any other
([close_over.rs](../src/builtins/close_over.rs)). Its block runs over a
dedicated per-call-tier region with no `outer` link; the tail value returns
homed there, and holders pin exactly that region plus any callable pins. The
block scope's outer is the innermost eternal-homed scope of the enclosing
chain — builtins and top-level definitions stay visible and contribute no
reach; every per-call binding arrives through capture. Block-local bindings
are invisible outside; only the tail escapes.

Capture kinds:

- **Identifier** (value or type channel): resolved at the form's step
  (parking on an in-flight placeholder via the standard resolve path) and
  relocated under the `Copy` verb — data rebuilt transitively, strings
  re-bumped, type handles copied by value.
- **Signature-shaped pattern** `(HELPER _)` / `(MAP _ USING _)`: names one
  full untyped bucket key — never a bare lead keyword, since dispatch
  registrations are not identifier bindings — and captures that registration
  pinned. `_` is the universal hole token.
- **Implicit close**: at block-scope build time, every dispatch registration
  and module binding in the per-call portion of the enclosing chain is
  copied in, pinned with its full transitive reach. Operator registrations
  count as dispatch registrations, and `USING` window scopes are part of the
  walked chain — their copied entries pin the module's region. The build
  parks on every visible in-flight claim in the chain, so what gets closed
  over never depends on scheduling. This is a build-time act, not call-time
  outward resolution — an escaped closure's body dispatches after its
  ancestors are dead, so nothing may resolve outward later.

A `USING` window's surfaced *registrations* and *modules* close implicitly by
that rule; a surfaced value member is data, so it is named in the capture list
like any other datum.

Flattening a chain into one scope keeps the innermost entry for each name,
token and operator probe alike. For the operator registry that is exact, not
lossy: resolution stops at the first scope holding the probe, so a use site
that could have reached an outer declaration does not exist, and the two never
meet inside the block. Handing both to the registry instead would raise its
one-chaining-mode-per-scope conflict — a rule about what a single scope's own
declarations may say, which shadowing scopes were never subject to — and a run
that reduces outside the block would refuse to compile inside it.

A closure defined inside the block is severed by construction: its captured
chain is the block scope, then the eternal tier.

EVAL is permitted; names resolve against the block scope, the captures, and
the eternal chain, and fail as unbound at that boundary otherwise.

### Nothing the block reads is homed in the caller

Severance removes the link that would keep the caller's frame alive, so every
piece of state the block reads *after* that frame retires must be homed
somewhere the block still names. Two placements follow, and both are
invariants of the form rather than incidental choices:

- **The body's working copies rest in the block's own region.** A block's
  statements arrive as raw AST and are frozen into working form, and the
  block's tail is read back from the block frame's own cart, which holds
  nothing of the caller — so freezing at the call site would leave that read
  dangling, and freezing into the eternal tier, the one other region the
  block scope names, would leave one copy per *evaluation* of the form alive
  for the rest of the run. Homing them in the cart makes both false. The cart
  does not exist yet at the step that decides the tail, so the body crosses
  the install as raw AST
  ([`fresh_cart_tail`](../src/machine/core/kfunction/block_tail.rs)) and the
  reinstalled step — running with the fresh cart as its own scope — freezes
  the working run at that cart's brand. Its cost is one extra scheduler hop
  per evaluation.
- **A block statement's result rests in the block's own region.** A statement
  binds in the block, so its terminal is destined at the block frame's region
  rather than at the consuming slot's cart
  ([harness.rs](../src/machine/execute/harness.rs)). Naming the cart would
  have the caller's region retain the block's while the block retains a
  capture back out of it — a ring neither side frees.

## Lazy close: the copy verb through callables

Deep-copying a function or module deep-copies the data it closes over: the
captured scope chain is rebuilt at the destination — data bindings relocated
under `Copy`, nested callables recursed, eternal-homed scopes referenced
verbatim — memoized per source scope and callable, so a recursive FN's
scope→function→scope cycle terminates and sibling closures sharing a
defining scope share the copy. The trigger is seam pricing, the same
copy-vs-pin shape as [value-substrates.md](value-substrates.md): copy when
the rebuild is small against what the pin would retain.

Because the copy can fire at an arbitrary escape crossing, it may meet an
environment still under construction: an unfinalized binding, or a captured
scope whose defining block has not closed. The copy then pins rather than
parks — `Pin` is always sound, and severance retries at a later priced
crossing, frame death at the latest, by which point the scope has closed.
No wait edge is ever added to the finalize walk, so the deadlock two
mutually-referencing in-flight environments would otherwise create is
unconstructible.

## Inferred capture

`CLOSE (block)` infers the capture list: the block's free value identifiers,
callables and modules being implicit already. `$(...)` inside such a block
is a structured error — EVAL resolves names dynamically, so inference is
impossible; `CLOSE OVER` is the EVAL-friendly form. `CLOSE OVER ()` is an
empty capture, distinct from inference.

## Open work

- [Lazy close](../roadmap/foundation/lazy-close.md) — the transitive
  callable copy and its pin-not-park downgrade rule; the liveness matrix
  ([liveness-matrix.md](../workgraph/design/liveness-matrix.md)) consumes it
  as its proactive consolidation lever.
- [Inferred CLOSE](../roadmap/foundation/inferred-close.md) — capture-list
  inference.
