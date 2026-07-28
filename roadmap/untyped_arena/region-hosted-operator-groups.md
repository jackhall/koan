# Region-hosted operator groups

The operator registry stores a sealed carrier like the other two binding tables,
so no scope owns a refcounted record outside the arena.

**Problem.** `Bindings` holds three tables. `data` and `functions` store
`Sealed`-shaped carriers — the value lives in a region, and the entry carries the
reach that proves it. `operators`
([bindings.rs](../../src/machine/core/bindings.rs)) stores
`HashMap<String, (Rc<OperatorGroup>, BindingIndex)>`: a refcounted record
allocated outside every region, under a second ownership regime the arena does not
see. `ScopeKind::Module` holds a second `Rc` clone of the same record
([scope.rs](../../src/machine/core/scope.rs)), so a `GROUP` body's scope owns a
strong reference to data no region hosts, and `Scope::nearest_group_context` hands
out a clone that outlives the borrow it came from — a resolved reference escaping a
scope, which the carrier discipline exists to prevent everywhere else.
`OperatorGroup` is lifetime-free owned data (a `HashSet<String>` of member keywords
plus a `ReductionMode` whose combiner is a *symbol*, not a resolved function), so
the `Rc` is sound today; the cost is structural rather than fault-capable. A
`GROUP` declaration installs one entry per nonempty subset of its members — the
per-group powerset — and each subset key clones the `Rc`, so registration cost is
refcount traffic proportional to `2^n`, and region death frees none of it: the
records die on the last `Rc` drop, on a schedule unrelated to the scope that
declared them.

**Acceptance criteria.**

- `Bindings.operators` maps a probe key to a sealed carrier plus a
  `BindingIndex`, the same entry shape the `data` and `functions` tables use.
- `OperatorGroup` records live in a region's arena; no `Rc<OperatorGroup>` exists
  in `src/`.
- `Bindings` stays lifetime-free — the entry carries no region borrow, exactly as
  the other two tables' entries don't.
- `ScopeKind::Module`'s group payload is a sealed carrier, and
  `Scope::nearest_group_context` answers under a pin rather than handing back a
  clone that outlives its borrow.
- A `GROUP` whose declaring scope's region has died is unreachable through the
  registry — the walk resolves nothing rather than reading a record kept alive by
  a stray refcount.
- A chain expression resolving a group declared in an ancestor scope records that
  ancestor's region in the resolved carrier's reach.
- The Miri audit slate is leak-free and UB-free.

**Directions.**

- Whether uniformity justifies the move — *open.* The record is lifetime-free by
  construction (holding the combiner as a symbol is what keeps it so), so
  region-hosting buys no reach proof the `Rc` lacks; the win is one ownership
  regime and death tied to the declaring scope instead of to a refcount. The
  counter-case is that an arena door exists solely for a record with no region
  borrow — the exact reason the door was deleted when the `Rc` landed.
  Settle this before plumbing: if the answer is no, the item closes as a
  documented decision instead of shipping.
- What the powerset keys share — *open.* Every subset key of one `GROUP` must
  resolve to the same record. Options: (a) one region allocation, each key holding
  a sealed carrier over the same pointee, so sharing is address identity as it is
  for `Rc::ptr_eq` today; (b) one allocation per key with the upsert's structural
  equality (`mode` + member set) as the sole identity rule, dropping `ptr_eq` from
  `write_operator_group`. Recommended: (a) — it keeps the cheap identity arm and
  makes the powerset install allocation-free past the first key.
- Which region hosts a builtin group — *decided.* The run-global root scope's
  region. `register_builtin_operator_groups`
  ([arithmetic.rs](../../src/builtins/arithmetic.rs)) seeds the comparison /
  additive / multiplicative groups into the root, which outlives every per-call
  region, so an inner scope's resolved carrier names an ordinary foreign member.
- How `nearest_group_context` is consumed — *open.* Its caller is the `OP`
  declaration path, which reads the mode to admit a heterogeneous `->` and writes
  the member's registry entry. Options: a closure-taking `with_group_context`, or
  returning a sealed carrier the `OP` binder opens under the pin it already holds.

## Dependencies

**Requires:**

- [Binding tables as witnessed carriers](binding-tables-witnessed-carriers.md) —
  establishes the sealed-carrier entry shape and the lifetime-free `Bindings` this
  third table would join.

**Unblocks:** none — leaf.
