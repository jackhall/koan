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
scope, which the carrier discipline exists to prevent everywhere else. A `GROUP`
declaration installs one entry per nonempty subset of its members — the per-group
powerset — and each subset key clones the `Rc`, so registration cost is refcount
traffic proportional to `2^n`, and region death frees none of it: the records die on
the last `Rc` drop, on a schedule unrelated to the scope that declared them. The
record's own shape blocks the move: `OperatorGroup` holds a `HashSet<String>` of
member keywords, which owns a heap table and runs `Drop`.

**Acceptance criteria.**

- `OperatorGroup`'s member set is a sorted slice of bump-hosted keywords, probed by
  binary search; the record is `Copy` and `Drop`-free.
- `OperatorGroup` records live in a region's bump; no `Rc<OperatorGroup>` exists
  in `src/`.
- `Bindings.operators` maps a probe key to a sealed carrier plus a
  `BindingIndex`, the same entry shape the `data` and `functions` tables use.
- `Bindings` stays lifetime-free — the entry carries no region borrow, exactly as
  the other two tables' entries don't.
- One allocation per `GROUP` backs every one of its powerset keys, so
  `write_operator_group`'s cheap identity arm stays an address compare and the install
  is allocation-free past the first key.
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

- *Whether the move is justified — decided.* Yes. An `OperatorGroup` is koan semantic
  data, and koan semantic data lives in a region's bump arena, under one ownership
  regime with death tied to the declaring scope rather than to a refcount.
- *Member-set representation — decided.* A sorted slice, not a hash set. Member counts
  are necessarily tiny — the powerset install is `2^n`, so ten members is already 1023
  keys — and binary search beats hashing at that size. It also makes the upsert's
  structural identity rule (`mode` plus member set) a slice compare, and each powerset
  subset key a sorted sub-slice rather than a rebuilt set. An `elsa` frozen collection
  is not an option for bump-hosted data: `FrozenIndexSet` owns an `IndexSet` inside an
  `UnsafeCell` and runs `Drop`, which a bump never runs — elsa earns its place as a
  `Region` *field* (the reach intern table), dropped by the region itself.
- *What the powerset keys share — decided.* One region allocation, each key holding a
  sealed carrier over the same pointee, so sharing is address identity exactly as
  `Rc::ptr_eq` gives it today.
- *Which region hosts a builtin group — decided.* The run-global root scope's
  region. `register_builtin_operator_groups`
  ([arithmetic.rs](../../src/builtins/arithmetic.rs)) seeds the comparison /
  additive / multiplicative groups into the root, which outlives every per-call
  region, so an inner scope's resolved carrier names an ordinary foreign member.
- *How `nearest_group_context` is consumed — open.* Its caller is the `OP`
  declaration path, which reads the mode to admit a heterogeneous `->` and writes
  the member's registry entry. Options: a closure-taking `with_group_context`, or
  returning a sealed carrier the `OP` binder opens under the pin it already holds.

## Dependencies

The member slice's element type is a bump-hosted `&'a str`, the representation tags and
keys already take ([design/value-substrates.md § Sectioned reach](../../design/value-substrates.md#sectioned-reach)):
a sorted slice of those, with no interning table behind it.

**Requires:**

- [The region bump door](../../workgraph/src/witnessed/bump.rs) — shipped substrate:
  `FoldedPlacement::fold_and_bump` is the public door group records and member slices
  land in.

**Unblocks:** none — leaf.
