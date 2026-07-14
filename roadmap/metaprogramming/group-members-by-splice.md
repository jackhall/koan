# Group members may arrive by splice

**Problem.** `GROUP` membership is decided entirely before evaluation: a
structural scan of the unevaluated body collects the top-level `OP` statements,
and the full nonempty powerset of the member set registers into the group's
child scope before a single body expression runs
([design/operators.md](../../design/operators.md),
[`group_def.rs`](../../src/builtins/group_def.rs)). An `EVAL` at the body's top
level has no scan-time content, so an `OP` it splices cannot join the group —
under splice semantics it would land in the group's child scope as a silent
non-member, satisfying dispatch for two-operand uses but never reducing a
mixed run under the group's mode.
[design/metaprogramming.md](../../design/metaprogramming.md) specifies late
member join: a spliced top-level `OP` is a member from the moment its `EVAL`
finalizes.

**Acceptance criteria.**

- A `GROUP` body containing an `EVAL` whose splice declares a top-level `OP`
  yields a group where that operator is a member: a mixed run pairing it with
  a scan-time member reduces under the group's mode, both in body expressions
  sequenced after the `EVAL` barrier and through a `USING` window over the
  group.
- The group's shared record and its registered subsets include the spliced
  member once the `EVAL` finalizes; body expressions sequenced before the
  barrier resolve as if the member did not exist.
- Mode consistency, operand typing, and the pairwise-only rule for
  heterogeneous members hold for a spliced `OP` exactly as for a written one —
  a spliced member conflicting with the group's mode is the same error.
- An `OP` spliced inside an `FN` within the body still declares in that `FN`'s
  per-call scope and joins no group.

**Directions.**

- *How the record grows — open.* (a) Extend the one shared
  [`OperatorGroup`](../../src/machine/model/operators.rs) record in place and
  register the incremental subsets covering the new member at EVAL-finalize;
  (b) rebuild the record and re-register the full powerset. Recommended: (a) —
  all existing subsets stay valid, and the record staying lifetime-free means
  the extension is a plain data update behind the existing registrar door.

## Dependencies

**Requires:**

- [EVAL splices in place](eval-splices-in-place.md) — a spliced member `OP`
  presupposes splicing.

**Unblocks:** none tracked.
