# Witness-derived binding

Fuse the bind leg with the value's witness so scope entry cannot state a
reach the value's borrows don't back.

**Problem.** Two coupled gaps at the leg where values enter scopes.
`Scope::bind_value(name, object, index, reach)` and
`Scope::resident_type_carrier(kt, reach, borrows_into_home)`
([scope.rs](../../src/machine/core/scope.rs)) accept a caller-supplied
`StoredReach` / home-bit with no tie to the value's actual borrows, and the
`omit` predicates on reach mints are equally free-form — a wrong reach, bit,
or omit under-pins, and a later read rebuilds a carrier from the stored lie.
Upstream, the move-in audits verify a value's borrows against the evidence
the caller *passed*, but nothing couples that evidence to what ultimately
pins the stored reference: the alloc and the pin are separate acts paired
only by call-site adjacency. Eighteen call sites ride the two methods —
`bind_value` ×4 ([nodes.rs](../../src/machine/execute/nodes.rs),
[let_binding.rs](../../src/builtins/let_binding.rs),
[branch_walk.rs](../../src/builtins/branch_walk.rs),
[kfunction/exec.rs](../../src/machine/core/kfunction/exec.rs)) and
`resident_type_carrier` ×14 across the builtins and dispatch. The same
free-form reach also over-pins silently: folding a region the value never
borrowed from passes every audit.

**Acceptance criteria.**

- Scope binding derives `StoredReach` and `borrows_into_home` from the bound
  value's carrier witness; no bind door accepts them as free parameters.
- The mint/pin and the store are one call: a fused door (the pattern of
  `Delivered::adopt_into` in
  [delivered.rs](../../workgraph/src/witnessed/delivered.rs)) replaces the
  alloc-then-bind adjacency pairing at every current `bind_value` /
  `resident_type_carrier` site.
- A bind cannot over-pin: the derived reach names exactly the regions the
  witness names, so bind-side reach over-approximation is closed by
  construction.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Door shape — open.* (a) The fused door consumes the delivery envelope
  (`Delivered` / `Sealed`) and reads the reach off its witness; (b) a typed
  reach token obtainable only from a witness, threaded to today's two-step
  mint-then-bind. Recommended: (a) — one act, nothing to thread.
- *`omit` predicates — open.* Whether reach-mint `omit` survives as an
  optimization the witness proves safe, or is deleted with the free-form
  mint.

## Dependencies

**Requires:** none — operates on the current veneer layer.

**Unblocks:** none tracked — soft ordering with
[Scheduler-owned lifetime tokens](scheduler-lifetime-tokens.md) and
[Fold-closure capture provenance](fold-closure-provenance.md) is noted in
those items.
