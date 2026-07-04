# Carrier-carrying spliced parts

The copy-free ending of guarantee 5 of
[design/scheduler-library.md](../../design/scheduler-library.md): a dep that
survives past its resolving step travels as its sealed carrier, never as a
bare relocated value.

**Problem.** Three dispatch finishes need a dep's value past the step in
which it resolves — owned deps cascade-free after the step — and each keeps
that copy as a bare `Carried` with its reach in a parallel channel: the
eager-subs finish
([ctx.rs](../../src/machine/execute/dispatch/ctx.rs)) writes
`ExpressionPart::Spliced(Carried)` into the working expression and threads
the matching `Sealed` carrier separately through `arg_carriers`; the
head-deferred finish
([head_deferred.rs](../../src/machine/execute/dispatch/head_deferred.rs))
relocates the resolved callable and deposits its reach via a separate
`fold_reach` onto the consumer scope; the FN signature splice
([fn_def/finalize.rs](../../src/builtins/fn_def/finalize.rs)) splices
resolved type slots as bare values. Because the value and its reach ride two
channels, embedding a dep without naming its reach stays expressible at these
sites — guarantee 5 holds there by per-site discipline
(`DepTerminal::relocate` paired with carrier threading), not by shape.

**Acceptance criteria.**

- `ExpressionPart::Spliced` carries the dep's sealed carrier — value and
  reach as one unit; a consuming decide or bind opens it at its own step
  brand, and no bare `Carried` rides the working expression across steps.
- The head-deferred finish classifies and applies the resolved head through
  its carrier; the callable's survival is the carrier (its witness the pin),
  not a relocated copy in the consumer region.
- The FN signature splice delivers resolved type slots as carriers, opened
  where the signature is assembled.
- `DepTerminal::relocate`'s sole remaining caller is the catch channel
  (`catch_continuation`).
- Existing tests and the Miri audit slate green.

**Directions.**

- *Cell representation — open.* (a) `ExpressionPart::Spliced(Sealed<CarriedFamily, FrameSet>)`
  directly — the cell is lifetime-free, so the `KExpression<'r>` family's
  layout-invariance is untouched, but every classifier that today matches on
  the spliced value must open the carrier at a brand; (b) a dedicated
  spliced-cell type bundling the carrier with a memoized `KType` so dispatch
  classification reads the type without an open (dispatch trusts the carried
  element type).
- *The `arg_carriers` channel — open.* Whether the slot-keyed `arg_carriers`
  side-channel collapses into the carrier-carrying cell itself (one delivery
  per argument), or stays as the body-facing projection built from the cells.

## Dependencies

**Requires:**


**Unblocks:** none tracked.
