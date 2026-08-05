# Typed expression value channel

**Problem.** Three answers rest on the claim that no expression reaching the value
channel borrows a region a holder can outlive: `object_cell_reach` calling an
expression's cell `Owned` and `retains_home` answering `false`, both in
[`KObject`](../../src/machine/model/values/kobject.rs), and the expression door
[`RegionBrand::alloc_expression`](../../src/machine/core/arena.rs) sealing its cell
with no member.
[`ProgramBrand`](../../src/machine/core/arena/frame.rs) types half of that: the
parse entry points take it, so parse output's eternal storage tier is checked at
every call site. The other half is discipline. `fn_def.rs`'s deferred
placeholder, `val_decl.rs`'s type wrapper and `op_def.rs`'s `bridge_body` each
build a node against an ordinary `RegionBrand` at their declaring scope, and are
correct only because each is unwrapped or sub-dispatched where it is built —
nothing in the type stops a fourth such site from wrapping one as
`KObject::KExpression`, and nothing would notice: `KExpression` is covariant, so
a node borrowing a per-call region coerces to any shorter lifetime and the borrow
checker sees nothing to object to. The three answers above would then be wrong
about a live value, under-pinning its producer region.

**Acceptance criteria.**

- Constructing a `KObject::KExpression` requires proof that the node's storage is
  eternal-tier: a value cell over a node built against a per-call brand is a
  compile error, not a discipline violation.
- `object_cell_reach`, `retains_home` and the expression door's own empty seal
  cite that proof rather than a flow argument, and
  [`ProgramBrand`](../../src/machine/core/arena/frame.rs)'s doc records no
  residual split.
- A runtime-synthesized node still reaches its own dispatch without being forced
  into eternal storage — the guard does not turn per-call AST into unbounded
  program-storage growth.
- `let_bound_list_of_call_produced_quotes_survives_every_producer_free` no longer
  sits on the koan Miri audit slate — the claim it pinned is compile-enforced.
- The Miri audit slate is green.

**Directions.**

- *Where the proof lives — decided.* At the arm level, not the node. `KExpression`
  is `Copy` and embedded by value in the cell, so the eternal-tier claim is about
  the **parts slice** the node borrows — and the four expression-holding
  [`ExpressionPart`](../../src/machine/model/ast.rs) arms (`Expression`,
  `SigiledTypeExpr`, `RecordType`, `QuotedExpression`) are the only conduits into
  the value channel. Those arm payloads become a `ProgramNode<'a>` (newtype over
  `&'a KExpression<'a>`, minted only under
  [`ProgramBrand`](../../src/machine/core/arena/frame.rs)), and
  `KObject::KExpression` takes a `ProgramExpression<'a>` (`Copy` newtype over the
  node). Every value-channel door then compiles its proof out of the matched arm,
  while the dispatch channel — `sub_dispatches`,
  [`WorkingExpression`](../../src/machine/model/ast/working.rs), the classifier,
  the structural cache — keeps carrying bare `KExpression`: the marker is consumed
  only where the claim is used, so it never goes viral and needs no erase point.
  Parser internals retype `RegionBrand` → `ProgramBrand` (they widened too early;
  the entry points already take `ProgramBrand`).
  Rejected: a node-level tier parameter (viral through dispatch), and the
  reach-as-data inversion (doors take the node's coverage as an operand) — region
  identity is not derivable from a bare node, so honest coverage would have to
  travel as envelopes through `Held`, `WorkingPart`, `NodeWork` and every
  container seal, a bind-channel re-currency that computes an empty coverage on
  every live path.
- *What the synthesized sites do — decided.* Most ride through unchanged:
  `fn_def.rs`'s placeholder and `val_decl.rs`'s wrapper build no marked arm;
  `fn_def/signature.rs`'s and `newtype_def.rs`'s re-wraps take their proof from
  the marked arm or `KObject` arm they matched it out of, via a re-host door (the
  node struct may bump at any brand — only the parts matter). Two sites fabricate
  marked arms from scratch and change shape instead: `op_def.rs`'s `bridge_body`
  builds in program storage — once per `OP` declaration, so growth is bounded by
  program size — which requires `ProgramBrand` to be reachable from the builtin's
  ctx; and `dispatch/constructors.rs`'s `single_value_cell`, whose multi-part
  wrap exists only to be dispatched, moves to the working form
  ([`WorkingExpression`](../../src/machine/model/ast/working.rs)) — the lane
  scheduler-synthesized nodes already dispatch through — rather than minting a
  marker or growing program storage per call.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none.
