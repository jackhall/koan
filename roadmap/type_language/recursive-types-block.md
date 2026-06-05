# `RECURSIVE TYPES` block for mutually-recursive nominals

A `RECURSIVE TYPES Name = (...)` block co-declares a group of mutually-recursive
nominal types as one `RecursiveSet` — the only lexical-order-compatible way to
name a cycle of two or more types.

**Problem.** With type-name resolution chain-gated to strict lexical order (no
`cutoff=None` carve-out), a forward type reference is a position error.
Self-recursion survives: a binder threads its own name into its body, a back-edge
(`RecursiveRef`) like `FN f = (… f …)`, not a forward reference. Mutual recursion
cannot — in `STRUCT A = (b :B)` / `STRUCT B = (a :A)` whichever is written first
forward-references the other, and a cycle of ≥2 names has no valid statement order.

**Design.** The block makes the group explicit and lexically scoped:

    RECURSIVE TYPES Pair = (
        STRUCT A = (b :B)
        STRUCT B = (a :A)
    )

- Body: a newline-separated sequence of ordinary `STRUCT` / `UNION` / `NEWTYPE`
  declarations (existing surface syntax, no special inner form).
- Every member name is in scope for every body inside the block (the threaded
  group), so a cross-reference lowers to a transient `RecursiveRef` and seals to a
  `SetLocal` index. Outside the block, strict lexical order applies.
- **Exiting the block guarantees resolution.** The block boundary is the seal
  point: on exit, every forward reference used inside it must name a member of the
  group. A `RecursiveRef` that resolves to no member is an error raised at the
  block's end, so no unresolved forward reference can escape — the forward
  placeholders the block grants are always discharged where they are declared.
- The seal at exit mints one `RecursiveSet` — the explicit seal trigger, retiring
  the implicit `detect_pending_cycle` SCC detection, the park-on-forward-placeholder,
  and the cross-member edge bookkeeping. The `RecursiveSet` value model already
  shipped (commit 640e767): `Rc`-owned, intra-set refs `SetLocal`, external refs
  `SetRef`, lift = `Rc::clone`.
- Member names (`A`, `B`) bind as ordinary type names in the enclosing scope; the
  group name (`Pair`) binds the `RecursiveSet` handle.

**Impact.**

- *One visibility rule.* Type names obey lexical order everywhere; the only
  cross-order resolution is the explicit, named, lexically-scoped block.
- *Declared, not detected.* The SCC is written down, so the runtime stops inferring
  it — `detect_pending_cycle` and its edge/park machinery retire.
- *No dangling forward reference.* Resolution is guaranteed at the block boundary,
  so a forward reference is either discharged into the set or a localized error —
  never a placeholder that survives into a sealed type.
- *Self-recursion needs no block* — the threaded self-name covers it.

**Directions.**

- *Surface — decided.* `RECURSIVE TYPES Name = (newline-separated decls)`.
- *Members — decided.* Any self-referenceable nominal: `STRUCT`, `UNION`, `NEWTYPE`.
- *Group handle — bind decided, use open.* `Name` binds the `RecursiveSet`, reserved
  for value-language cycle construction; the namespace form (`Name.A`) is unspecified.

## Dependencies

**Requires:**

- [Lookup protocol](../../design/typing/lookup-protocol.md) — the chain-gated
  `visible(idx, cutoff)` walk the block scopes its threaded group within.

**Unblocks:**

- [Retire the lexical-visibility carve-outs](../refactor/position-dependent-type-resolution.md)
  — the block is the mutual-recursion mechanism that lets the elaborator chain-gate
  type names with no `cutoff=None` exception.
