# A witnessed layer laid in the memo's home

A witnessing activation's witnessed layer laid once, where its decision memo
keeps it, and not in the frame's region.

**Optional.** An optimization nothing needs yet.

**Problem.** A witnessing activation
([`activated`](../../src/program/activation_store.rs)) lays both its
witnessed layer and its binding layer in the frame's own region, and the
decision memo's miss ([`extended`](../../src/scope/memo.rs)) then relays both
into the memo's home, the region where the frame's result is built
([activations](../../src/build/README.md#activations)), so the witnessed
layer rests twice. A witnessing frame's region is heavier than a closed
walk's, which raises the price of pinning it: a call returning its argument
hands back its storage only for a list of 256 elements in
`a_call_returning_its_argument_hands_back_its_storage`
([`programs.rs`](../../src/program/tests/programs.rs)), where 64 sufficed
under a closed walk, so a returned list of roughly 64 to 255 elements is
copied rather than handed back.

**Acceptance criteria.**

- A witnessing event lays its witnessed layer in its memo's home alone, and
  the frame's region holds only the binding layer, whose parent is the
  home's witnessed layer.
- `a_call_returning_its_argument_hands_back_its_storage` passes at 64
  elements.
- The benchmark in the [audit scaffolding](../../audit/README.md) reports a
  witnessing frame's region bytes per hop equal to the same loop's walked
  closed.
- The memo law holds every reuse equal to a decision made from scratch, and
  every program prints as before.

**Directions.**

- *Where the witnessed layer is laid — open.* Through the home's writer
  during the activation, or in scratch and relaid once by the memo's lay.
  Recommended: the memo's lay, which already relays into the home and adopts
  the types the layer names.
- *The order of the memo write and the binding layer's relay — open.* The
  binding layer's parent must be the home's copy, so the memo's lay runs
  before the binding layer is relaid into the frame's region, or the binding
  layer is re-parented after it.
- *Soundness — decided.* An event witnesses only where its memo has room to
  keep the witnessed layer (`HalfHeader::memo_room`), so a home always keeps
  what a witnessing event lays.

## Dependencies

**Requires:** none — its substrate has shipped.

**Unblocks:** none — a leaf.
