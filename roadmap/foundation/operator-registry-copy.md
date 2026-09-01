# Operator registrations in a copied environment

Teach the environment copy
([lazy-closures.md § Lazy close](../../design/lazy-closures.md#lazy-close-the-copy-verb-through-callables))
to rebuild a scope's operator registry, so a chain carrying one stops pinning.

**Problem.** The readiness gate
([`Scope::is_copy_ready`](../../src/machine/core/scope.rs)) declines any scope
holding an operator-registry entry ([`Bindings::has_operators`](../../src/machine/core/bindings.rs)), so one such
link pins the whole crossing and the copy engine
([`fill_scope`](../../src/machine/core/scope/copy.rs)) asserts the table is
empty rather than copying it.

That is self-defeating for `CLOSE`. The capture plan
([`close_over.rs`](../../src/builtins/close_over.rs)) flattens **every**
per-call operator registration visible on the enclosing chain into the block
scope — unconditionally, with no filter for what the block applies — so a
`CLOSE` block written anywhere under a per-call `OP` or `GROUP` declaration
installs the exact entry that makes its own block scope unready. The form whose
whole purpose is to sever captures is the one that cannot be consolidated. A
top-level or builtin operator is not flattened (the walk stops at the eternal
tier), so the bite is narrow but not rare: it lands on any program that
declares an operator inside a function and closes over something in it.

The gate declines for a real reason. An entry is
`OperatorEntry { index, address, declaration: &'a str, sealed }`: the
`&'a OperatorGroup<'a>` record its seal names is resident in the declaring
scope's region, and both the `address` identity arm and the bumped
`declaration` text belong to that region too. Copying the map wholesale would
leave the rebuilt scope pointing into the source — the retention the copy
exists to release.

Most of the machinery already exists, and on easier terms than the callable
rebuild that shipped. `Region::birth_operator_group` builds a record at a
destination region under a `for<'b>` yoke brand, and `adopt_operator_registration`
installs a delivered group into a fresh scope — the pair `CLOSE OVER`'s own
seeding uses. What a source record has to give up to be re-born is
`member_symbols()` and `mode()`, both lifetime-free plain data, so unlike a
captured scope they can be read under the source borrow and carried into the
fold without a nested transfer.

**Acceptance criteria.**

- A captured chain whose scopes hold operator registrations is copy-ready and
  consolidates: the readiness gate no longer names operators, and `fill_scope`
  copies the table instead of asserting it empty.
- A closure escaping a `CLOSE` block that flattened a per-call operator applies
  that operator identically after the producer frame dies, and holds one region
  rather than the producer chain.
- A `GROUP`'s powerset entries copy to entries over **one** rebuilt record: the
  copied scope's entry count equals the source's, and every subset entry
  resolves to the same record address.
- The copy preserves the upsert's decisions — a registration that was a silent
  no-op against the source table is one against the copy, and a chaining-mode
  conflict is still a conflict — though a rebuilt record's `address` differs by
  construction.
- The rebuild is priced: an operator table's copy weight enters the per-scope
  [`binding_copy_cost`](../../src/machine/core/bindings.rs) memo, so the chooser
  pays for what it now copies rather than reading a cost that ignores it.

**Directions.**

- *Re-birth per record, share per entry — open.* Memoize source record address
  → rebuilt record across one scope's fill, so a `GROUP`'s powerset costs one
  birth. Without it the copy builds 2ⁿ−1 distinct records where the source has
  one, and the third criterion fails.
- *Reading a source record — likely settled.* `member_symbols()` + `mode()`
  under the source borrow, then `birth_operator_group` at the fold brand. The
  alternative — routing the record through a nested `transfer_into` over
  `OperatorGroupFamily`, mirroring how the callable rebuild routes a captured
  scope — buys nothing here, since neither output borrows the source region.
- *Registry-free insert door — mechanical.* `write_operator_group` takes
  `RunRegistries` only to render a conflict label, and a fresh table cannot
  conflict; mirror `insert_copied_overload`'s registry-free door for the same
  reason it exists.
- *Narrower alternative — open.* Filter `CLOSE`'s flatten to the operators the
  block actually applies. That shrinks the problem without closing it — a block
  that genuinely uses a per-call operator still pins — so it is a complement to
  the rebuild, not a substitute.

## Dependencies

The sibling item [Callable copy tuning](callable-copy-tuning.md) owns the other
levers the first callable-copy seam leaves unpulled; the operator gate is
carved out here because its rebuild is a self-contained table copy rather than a
pricing decision. `Module`-kinded scopes decline on a group record too, so
whichever lands first informs the other.

**Requires:** none — the copy engine it extends is shipped.

**Unblocks:** none tracked yet.
