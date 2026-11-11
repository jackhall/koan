# The overlap check over a parametric registration

A user overload a builtin always beats, refused at load whether its signature
is closed, quantified or rigid.

**Problem.** [The overlap check](../../src/dispatch/README.md#the-overlap-check)
reads a registration's load-time expression shape, and checks only a closed,
unquantified one. Two kinds of registration escape it:

- **A quantified one is never checked.**
  `EXPR FOR ALL #[Elt] #(x :Elt + y :Elt) -> Str = #("mine")` loads beside the
  builtin `+`, which wins every call the two share: `PRINT (1 + 2)` prints `3`.
- **A rigid one holds the callee.** A callee whose signature names its
  encloser's type name reads the name rigid, and its overlap check is re-run
  under the binding that closes it, leaving an undecided mark of a holding
  kind on the registration. Every unit of the callee holds wherever the name
  is witnessed, so under an encloser bound to a novel type each hop, such a
  callee born in a loop's body walks at every hop, though its questions take
  the same answers
  ([activations](../../src/build/README.md#activations)).

**Acceptance criteria.**

- The quantified `+` above refuses the load with `ShapeError::Overlaps`, as a
  closed overload meeting the builtin `+` does.
- A quantified registration no binding within its group's bounds makes
  overlap a builtin loads: `EXPR FOR ALL #{Elt: Str} #(x :Elt - y :Elt) -> Str`
  beside the builtin `-` over `Number`.
- A callee whose signature names its encloser's type name, born in a loop's
  body under an encloser bound to a novel type each hop, decides its units
  once per answer its questions take, not once per hop: a test counts its
  walks.

**Directions.**

- *Which bindings a quantified registration is checked at — open.* Refused
  where some binding within its group's bounds makes every slot pair meet a
  builtin overload's above `Never`, the closed rule read over the bounds; or
  only where every binding does. Recommended: some binding, which the `+`
  criterion assumes, since the builtin wins every call at that binding.
- *What a rigid registration's check reads — open.* The rigid name through its
  bound, as a quantified registration's group is read, so the check decides at
  load and leaves no mark; or the check kept per binding with its mark holding
  the registration alone, not the callee's units. Recommended: through the
  bound, one rule for both kinds.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
