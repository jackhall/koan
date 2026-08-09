# Union-typed carrier slots for builtin signatures

Let a builtin signature slot carry a union of carrier types, so one overload
admits every carrier spelling of a type slot instead of one overload per
combination.

**Problem.** Builtin slots are matched under two regimes. Value-typed slots go
through type satisfaction, where `KType::Union` already works. The carrier
types — `KEXPRESSION`, `SIGILED_TYPE_EXPR`, `RECORD_TYPE`, `PROPER_TYPE`,
`IDENTIFIER` — are matched *structurally*, by exact `(KType constant,
part-kind)` pairs in `lazy_eager_indices` and `classify_for_pick`
([`pick.rs`](../../src/machine/core/kfunction/pick.rs)) and at capture time in
`held` ([`ast.rs`](../../src/machine/model/ast.rs)). A `Union`-typed slot falls
through the structural match into the speculative-eager arm, which
sub-dispatches the part and type-checks the result — raw capture is lost, so
the deferral machinery a carrier slot exists for never engages. Carrier
combinations must therefore be enumerated as separate overloads:
[`fn_def.rs`](../../src/builtins/fn_def.rs) hand-writes 4 of its 2×3 signature
× return matrix (the record-schema × constructed-return cell falls through
every overload), and [`op_def.rs`](../../src/builtins/op_def.rs) brute-forces
its full operand × result matrix with a double registration loop (10
registrations). Every new carrier grows the matrices multiplicatively.

**Acceptance criteria.**

- A builtin slot typed as a union of carrier types admits a part that any
  member admits, with the matching member's capture semantics — a
  `SigiledTypeExpr` part in a `union_of(of_kind(ProperType),
  SIGILED_TYPE_EXPR)` slot is captured raw, not eager-sub-dispatched.
- Registration rejects a union carrier slot whose members are not pairwise
  part-kind-disjoint (`Union` identity is order-blind, so admission must be
  deterministic without member order).
- Registration rejects a `KEXPRESSION` member inside a union carrier slot, so a
  slot's eager/lazy classification
  ([`lazy_eager_indices`](../../src/machine/core/kfunction/pick.rs), which keys on
  the slot's own ktype) stays unambiguous under unions.
- Unit tests exercise a union carrier slot through strict admission, relaxed
  admission, and slot capture.

**Directions.**

- *Match distribution — decided.* Distribute the structural `(ktype,
  part-kind)` match over union members at the enumerated sites —
  `lazy_eager_indices`, `classify_for_pick`'s bare-name arm, and `held` — and
  sweep the carrier constants' remaining consumers for any other site that
  must distribute.
- *Determinism — decided.* Pairwise part-kind-disjoint members, enforced at
  registration, rather than a member-precedence rule.
- *Binder-mask stance — decided.* Forbid `KEXPRESSION` union members instead
  of extending the mask derivation; the parse-time bare-form wrap in
  [Bare parenthesized return annotations](function-typed-return-annotations.md)
  keeps `KEXPRESSION` out of type slots, so no union ever needs it.

## Dependencies

**Requires:** none — a self-contained dispatcher generalization.

**Unblocks:**

- [Bare parenthesized return annotations](function-typed-return-annotations.md)
  — its overload-matrix collapse registers union-typed return and operand
  slots.
