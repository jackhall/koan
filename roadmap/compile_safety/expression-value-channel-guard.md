# Typed expression value channel

**Problem.** Three answers in
[`KObject`](../../src/machine/model/values/kobject.rs) — `resident_in_visiting`
admitting a `KExpression` unconditionally, `object_cell_reach` calling its cell
`Owned`, `retains_home` answering `false` — rest on the claim that no expression
reaching the value channel borrows a region a holder can outlive.
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
- `resident_in_visiting`, `object_cell_reach` and `retains_home`'s expression arms
  cite that proof rather than a flow argument, and
  [`ProgramBrand`](../../src/machine/core/arena/frame.rs)'s doc records no
  residual split.
- A runtime-synthesized node still reaches its own dispatch without being forced
  into eternal storage — the guard does not turn per-call AST into unbounded
  program-storage growth.
- The Miri audit slate is green.

**Directions.**

- *Where the proof lives — open.* (a) A branded wrapper type
  (`ProgramExpression<'a>`) that only `ProgramBrand`'s door mints, with
  `KObject::KExpression` taking it instead of a bare `KExpression`; (b) a
  lifetime-brand parameter on `KExpression` itself, distinguishing the tier at the
  node rather than at the cell; (c) a `Tier` type parameter carried by the node.
  Recommended: (a) — it confines the change to the value-channel constructors and
  keeps `KExpression` a single node type the shared readers already serve.
- *What the synthesized sites do instead — open.* (a) Leave them at the ordinary
  brand and route them through the working form
  ([`WorkingExpression`](../../src/machine/model/ast/working.rs)), which is where
  each already ends up; (b) build them in program storage under a size bound.
  Recommended: (a) — a node built to be dispatched is already the working form's
  business.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none.
