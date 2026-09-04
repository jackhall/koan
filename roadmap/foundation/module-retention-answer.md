# A module's retention answer is conservative

`retains_home` answers a module's residence question the exact way it answers a callable's — by
comparing the region its scope lives in against the region under release — instead of retaining
unconditionally.

**Problem.** [`retains_home`](../../src/machine/model/values/kobject.rs) is the release question
every copying relocation asks: does this value still borrow the region it came from? A `KFunction`
gets an exact answer, `std::ptr::eq(function.captured_scope().region(), home)`, on the grounds that
a callable's residence *is* its captured scope's region — the birth door derives the destination
from the scope, so the two cannot come apart. A `KObject::Module` gets the constant `true`.

The stated reason for the constant is that a module's child scope may hold member seals borrowing
some region other than the one the module lives in — an ascription view's replayed seals still name
the source module's — and that reach is not recoverable from the value. But that is equally true of
a callable's captured chain, and the callable arm answers residence anyway: the doc's own rule is
that *only residence decides whether the reference survives a release*, with further reach the
region graph's business, not this predicate's.

What changed is that a module's residence became as pinned as a callable's. Every module is now
born through `Module::alloc_at_child_scope` ([module.rs](../../src/machine/model/values/module.rs)),
so the value lives in its child scope's region by construction and the two cannot come apart either.
The conservative arm is a leftover from when they could.

The cost is a region the copy declines to release on every module-shaped relocation whose module
was resident elsewhere — the census the sibling item measures at 2/3/5 regions held against 1/1/1
for a bare `OP` reads the pin from the binding, but the retention answer is what makes the release
unaskable.

**Acceptance criteria.**

- The `Module` arm of `retains_home` compares the module's own scope region against `home` and is
  exact on the same terms the `KFunction` arm is, with the comment stating that ground rather than
  the conservative one.
- A relocation that moves a module value out of a region the module was not resident in releases
  that region, and the allocation census records the drop.
- The tightness audit and the Miri slate cover a module whose members reach a region other than its
  own across such a release, so the narrower answer is pinned against exactly the case the
  conservative one was guarding.

**Directions.**

- *Where the module's region is read — open.* `Module::child_scope().region()` is the direct
  reading; whether the value should instead carry its home region beside the scope, the way a
  substrate carries its stored reach, depends on whether any other caller wants the same fact.
- *Scope of the change — open.* [`carrier_witness.rs`](../../src/machine/core/carrier_witness.rs)
  routes its `is_home` claim through the same predicate, so the narrower answer reaches the witness
  channel too and its audit shapes are part of the acceptance surface.

## Dependencies

The sibling item [Module scope consolidation](module-scope-consolidation.md) owns the *other* half
of a module's pin — teaching the environment copy to rebuild a module's scope so the value can be
rebuilt at all. This item does not need it: a module that never rebuilds still gets asked whether it
retains the region it is leaving.

**Requires:** none — the per-node residence work that pinned a module to its child scope is shipped.

**Unblocks:** none tracked yet.
