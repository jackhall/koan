# Union-typed carrier slots for builtin signatures

Let a builtin signature slot carry a union of carrier types, so one overload
admits every carrier spelling of a type slot instead of one overload per
combination.

**Problem.** Builtin slots are matched under two regimes. Value-typed slots go
through type satisfaction, where `KType::Union` already distributes at every
arm (`accepts_part` / `accepts_carried` in
[`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)).
The exact carrier constants — `IDENTIFIER`, `NAME_TOKEN`, `TYPE_NAME_TOKEN`,
`SIGILED_TYPE_EXPR`, `RECORD_TYPE` — are matched *structurally*, by
exact-constant checks at three sites: strict admission (`slot_admits_strict`
in
[`resolve_dispatch.rs`](../../src/machine/execute/decide/resolve_dispatch.rs)),
the bare-name auto-wrap exclusion (`classify_for_pick` in
[`pick.rs`](../../src/machine/core/kfunction/pick.rs)), and bind-time capture
(`resolve_for` in [`ast.rs`](../../src/machine/model/ast.rs)). A `Union`-typed
slot falls through those exact checks into the speculative-eager arm, which
admits the part for eager sub-dispatch and type-checks the result — raw
capture is lost, so the deferral machinery a carrier slot exists for never
engages. (Scheduler-side raw keeping is not the gap: the seal-time
[`LAZY_SLOT_SPECS`](../../src/machine/model/lazy_slots.rs) stamp already
records raw kinds per slot index; only its registration-consistency pin must
learn to read union members.) Carrier combinations must therefore be
enumerated as separate overloads:
[`fn_def.rs`](../../src/builtins/fn_def.rs) hand-writes 4 of its 2×3 signature
× return matrix (the record-schema × constructed-return cell falls through
every overload), [`op_def.rs`](../../src/builtins/op_def.rs) brute-forces
its full operand × result matrix with a double registration loop (10
registrations), and [`newtype_def.rs`](../../src/builtins/newtype_def.rs)
registers one repr overload per carrier. Every new carrier grows the matrices
multiplicatively.

**Acceptance criteria.**

- A builtin slot typed as a union of carrier types admits a part that any
  member admits, with the matching member's capture semantics — a
  `SigiledTypeExpr` part in a `union_of(TYPE_NAME_TOKEN,
  SIGILED_TYPE_EXPR)` slot is captured raw, not eager-sub-dispatched.
- Raw capture is a property of exact carrier members (`IDENTIFIER`,
  `NAME_TOKEN`, `TYPE_NAME_TOKEN`, `SIGILED_TYPE_EXPR`, `RECORD_TYPE`) only:
  an `of_kind(…)` union member is an ordinary eager member, and no raw-capture
  semantics ride on it.
- Registration rejects a union carrier slot whose members are not pairwise
  capture-footprint-disjoint — the raw part shapes each member claims, with an
  `of_kind(ProperType)` / `of_kind(AnyType)` member counting the `Type`-token,
  `:(…)`, and `:{…}` shapes it lowers or shape-admits (`Union` identity is
  order-blind, so admission and capture must be deterministic without member
  order).
- Registration rejects a `KEXPRESSION` member inside a union carrier slot: a
  `(…)` group is the eager sub-expression shape, so a CODE-capturing union
  member would make the seal-time raw-kind derivation
  ([`lazy_slots.rs`](../../src/machine/model/lazy_slots.rs)'s
  registration-consistency pin) and group staging ambiguous.
- Unit tests exercise a union carrier slot through strict admission, relaxed
  admission, and slot capture.

**Directions.**

- *Match distribution — decided.* Distribute the structural exact-constant
  match over union members at the live sites — `slot_admits_strict`
  (shape-only strict admission via the matching raw member, with the
  speculative-eager guards distributed), `classify_for_pick`'s auto-wrap
  exclusion, `resolve_for`'s capture arms, and the spec⟺registration `kind_of`
  derivation in
  [`lazy_slots/tests.rs`](../../src/machine/model/lazy_slots/tests.rs) — and
  sweep the carrier constants' remaining consumers for any other site that
  must distribute.
- *Determinism — decided.* Pairwise capture-footprint-disjoint members,
  enforced at registration, rather than a member-precedence rule.
- *Binder-mask stance — decided.* Forbid `KEXPRESSION` union members instead
  of extending raw CODE capture to unions; the parse-time bare-form wrap in
  [Bare parenthesized return annotations](function-typed-return-annotations.md)
  keeps `KEXPRESSION` out of type slots, so no union ever needs it.

## Dependencies

**Requires:** none — a self-contained dispatcher generalization.

**Unblocks:**

- [Bare parenthesized return annotations](function-typed-return-annotations.md)
  — its overload-matrix collapse registers union-typed return and operand
  slots.
- [Type-slot resolution untangle](type-slot-resolution-untangle.md) — its
  deferral slots spell raw capture as unions of exact carrier members.
