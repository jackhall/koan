# EVAL splices in place

**Problem.** [`EVAL`](../../src/builtins/eval.rs) tail-dispatches its operand
AST in a fresh `CallFrame`, mirroring `MATCH`'s per-call frame: free names
resolve against the surrounding lexical scope, but body-introduced bindings
don't leak
([design/expressions-and-parsing.md](../../design/expressions-and-parsing.md)).
An assembled declaration run through `EVAL` therefore registers into a frame
that is discarded when the `EVAL` returns — `EVAL` cannot declare anything, and
the language has no route at all for installing a runtime-assembled `FN`, `OP`,
or `LET`. The confinement is not incidental: a block's top-level expressions
evaluate concurrently, so an `EVAL` that bound names into the enclosing scope
would give its siblings a scheduler-order-dependent view of those bindings.
[design/metaprogramming.md](../../design/metaprogramming.md) specifies the
replacement semantics: splice in place, sequenced by a block barrier.

**Acceptance criteria.**

- `EVAL q`, where `q` holds a declaration (`LET`, `FN`, `OP`), registers that
  declaration in the scope enclosing the `EVAL`: a later sibling expression in
  the same block resolves the binding, calls the function, or reduces a run
  with the operator.
- Any block-level expression containing an `(EVAL …)` head anywhere in its
  tree is a barrier: every later expression in the same block parks on it via
  dependency edges installed at submission, and a test placing the use before
  the declaration-splice in program order observes the same outcome under any
  scheduler order.
- Expressions earlier in the block than the barrier never observe the splice's
  bindings.
- An `EVAL` nested inside a sub-expression hoists the barrier to its containing
  block-level expression; an `EVAL` inside an `FN` body sequences the remainder
  of that body per call and nothing outside it.
- `EVAL` evaluates to whatever the spliced AST evaluates to, and free names in
  the spliced AST resolve against the `EVAL` site's scope; a non-`KExpression`
  operand remains a structured `TypeMismatch`.
- `EVAL` has one semantics in every position — module body, nested argument,
  `FN` body, `GROUP` body — with no statement/expression distinction.
- Splices obey the nested-binder position rule exactly as hand-written source
  does: a spliced binder in an eagerly evaluated value position raises the
  same structured `NestedBinder` error, so "`EVAL q` is exactly as powerful as
  writing `q`'s content in place" holds — both sides reject alike. (The
  pre-splice-in-place frame-local behavior is pinned by
  [`eval.rs`](../../src/builtins/eval.rs) tests
  `eval_spliced_let_is_frame_local` /
  `eval_spliced_let_in_argument_position_runs_frame_local`, which this item
  supersedes.)

**Directions.**

- *Barrier mechanism — decided.* Plain dependency edges from each later sibling
  to the barrier expression, installed at submission time off the parse-static
  `(EVAL …)` head shape. No placeholder registry entries and no wildcard park
  keys: the park target is a known node.
- *Frame handling for the splice — open.* (a) Evaluate the spliced AST directly
  in the enclosing frame, retiring the `MATCH`-mirroring fresh frame; (b) keep
  a child frame and forward its registrations to the enclosing scope through
  the existing deferred-write channels. Recommended: (a) — splice semantics
  *is* evaluation at the site, and a forwarding frame reintroduces the
  dual-write seam the scope docs work to avoid.

## Dependencies

**Requires:** none — foundation for the metaprogramming project.

**Unblocks:**

- [Group members may arrive by splice](group-members-by-splice.md) — a spliced
  member `OP` presupposes splicing.
