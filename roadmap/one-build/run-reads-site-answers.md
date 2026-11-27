# The run reads site answers

A run cell performing what the build answered at a site, without reading the
site's syntax again.

**Problem.** The evaluator is handed a node's syntax and re-derives what the
node is at every step, once per delivered part: `form`
([`evaluate.rs`](../../src/dispatch/evaluate.rs)) re-matches the builtin
shape and counts parts and keywords, `slots` rebuilds the list of wanted
operands, and a `MATCH` re-reads each arm's guard. What the build answered
there — the selected candidate, the chosen arm, the route of a tail, the type
a retype stamps, an ascription settled — sits in tables beside the syntax,
joined back by a search on the part's address
([`sites.rs`](../../src/scope/shape/sites.rs)), which one keyworded call
makes several times. Each expression shape is written twice, once in
[the walk](../../src/dispatch/walk.rs) over static types and once in the
evaluator over values, sharing the classifier and the leaf judgements and
little else: a read's rules are `walk/lookups.rs` and `lookup.rs`, a
`MATCH`'s `walk/arms.rs` and the evaluator's own.

**Acceptance criteria.**

- A run cell evaluating a site reads its `SiteAnswer`
  ([the binding graph](../../design/binding-graph.md#vocabulary)) and
  performs it; a test counts classifications of a node across a loop and
  counts none after the build.
- A frame finds each `SiteAnswer` in the `BoundInstance` of the
  `BindingNode` that owns the site's question, held by the frame from the
  bind site on, and reaches it without a search by address.
- Each expression shape's rule is stated once, and the build's reading over
  static types and the run's over values both come from it; `lookups.rs` and
  `lookup.rs` no longer hold parallel classifications.
- Every program's output equals its output before the change.

**Directions.**

- *The grain of what a run cell performs — open.* A `SiteAnswer` per site
  laid on the shape, the body runner walking units as it does; or a sequence
  per `BindingNode` that the body runner performs in order. Neither alone
  fits: a `BindingNode` spans bodies and a body spans `BindingNode`s, so
  what a frame performs is its body's sites, each answered from the instance
  of whichever node owns it. Decide with the graph in hand.
- *One rule per expression shape — open.* A rule generic over what it reads,
  a static type or a value, instantiated twice; or a rule stated as data both
  readers interpret.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
