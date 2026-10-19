# Views settled from content

A `:|` over a keyed module parameter typed once per binding, from the module's content.

**Problem.** An opaque view's carriers are keyed on the view's content: the operand's bare knot
member's digest, the signature and what the fit solves
([`view.rs`](../../src/knot/module/view.rs), `opaque` and `ModuleContent::view`). A
specialization's binding carries types alone, each keyed argument's carried type and the type a type
value names ([`KeyedTypes`](../../src/values/specialization.rs)), and the static pass reads a
parameter's content tree as a leaf, so no resumption can compute a view's carriers over a
parameter. The load classes such a `:|` invariant when its module parameter keys, yet the frame
mints the view's types each hop. A composite over those carriers mints with them. Today each frame
then copies them into its specialization's branch so the next hop finds them
([`evaluate.rs`](../../src/dispatch/evaluate.rs), the `:|` arm and `warmed`); activation typing
removes those copies, so a frame under a specialization mints them into its own scratch every hop,
and re-derives the verdicts over them.

**Acceptance criteria.**

- A `:|` whose operand is a keyed module parameter read bare has its view's types — each carrier,
  the view's signature, what the fit solves and each coerced member's type — settled in the
  specialization under the binding, and a frame under that specialization interns none of them.
- A composite site whose parts read only such views has its own memo type settled the same way.
- The view a frame builds there carries exactly the handles its specialization settled, which a debug
  build asserts.
- Verdicts over those types that the resumption reaches stay in the specialization across hops and
  callers.
- A view through a member chain of a parameter, and a nested re-view at an application that leaves a
  parameter open, still mint at run.

**Directions.**

- *What a binding carries for a module argument* — open. Recommended: the bare member's digest
  beside the type the frame's retype stamps, read by the asker at the seek.
- *What a view fact waits on* — open. The parameter's content as an unknown of its own, bound at
  entry with the digest; or every view fact over a keyed parameter leaf seamed at the entry and
  re-run whenever that parameter is bound.
- *Where the view's types are computed* — open. In the static pass's resumption, with the parameter's
  tree read as closed at the digest; or by the specialization's step calling the view door's type
  half directly at each such site after the resumption.

## Dependencies

**Requires:**

- [Activation typing](activation-typing.md) — the specialization's step runs a resumption, and frames read what it settles.

**Unblocks:** none.
