# User-definable n-ary operators

The reduction pre-pass that evaluates a recognized operator run — reducing the
run's operands by the mode its group declares (unary, fold, or pairwise).

**Problem.** An operator run is recognized but cannot yet *evaluate*. A
slot-led run of two or more operators (`a + b + c`, `A | B | C`) parses to the
cached `OperatorChain` shape (see
[expressions and parsing](../../design/expressions-and-parsing.md)), and its
operator group resolves through the per-scope operator registry walked by
[`resolve_operator_group_with_chain`](../../src/machine/core/scope.rs) (see
[the lookup protocol](../../design/typing/lookup-protocol.md)). But the
`OperatorChain` dispatch arm in
[`operator_chain.rs`](../../src/machine/execute/dispatch/operator_chain.rs)
terminates at an explicit reduction seam — a `"operator-chain folding not yet
implemented"` error — because nothing yet reduces a recognized run to a value.

**Acceptance criteria.**

- A recognized operator run evaluates to a value: the reducer reads the
  resolved [`OperatorGroup`](../../src/machine/model/operators.rs) and reduces
  the operands by the mode it declares.
- `a < b < c` and `1 <= x < 10` evaluate through the pairwise mode and yield a
  single boolean result.
- A run spanning two groups (`a + b * c`) resolves as a registry miss, and the
  parenthesized form `a + (b * c)` evaluates.
- A `|` run over [anonymous structural
  unions](../type_language/anonymous-unions.md) produces a union value, with a
  unary-mode `|` building the whole union in one pass.

**Directions.**

- *Partition, not precedence — decided.* There is no global precedence
  ordering. Operators partition into groups; a run within one group reduces, and
  a run spanning groups is the cross-group registry miss the user resolves with
  explicit parentheses. Relative-precedence tiers are not part of the model.
- *Reduction modes — decided.* A group declares one mode. **Unary** hands the
  operator's body the whole operand list (`< [a b c]`). **Fold left / fold
  right** reduce a binary body over the run. **Pairwise** maps each adjacent
  pair through its per-pair binary body and folds the results with a combiner.
  These are four peer modes, not a hierarchy — unary hands the body the raw
  operand list, while fold and pairwise build the reduction from a binary body.
  The surface that declares each is owned by
  [user-defined operator modules](user-defined-operator-modules.md).
- *Registry representation — decided.* A registry entry resolves to an
  [`OperatorGroup`](../../src/machine/model/operators.rs) carrying the member
  keyword set and a reduction mode — unary, fold-left, fold-right, or pairwise. A
  pairwise group additionally carries the **combiner** (the function value the
  per-pair results fold through) and its fold direction. The per-step bodies are
  not in the registry: the reducer resolves each binary or list body from the
  function bucket by keyword, so the registry holds only *how* a run reduces —
  the mode, the grouping, and the pairwise combiner — not what each step
  computes.
- *Mixed-operator runs — decided.* Fold and pairwise admit different operators
  from one group in a single run — `a + b - c`, `1 <= x < 10` — each position
  applying its own operator, read off the resolved `OperatorGroup`. Unary is
  homogeneous: one operator over the list.
- *Unary prefix and infix coincide — decided.* `< [x1 x2 …]` (a head-keyword
  applied to a list) and `x1 < x2 …` (the slot-led `OperatorChain`) are one
  surface for a unary operator: the infix run collects to the same operand list
  the prefix form passes, and both dispatch as an ordinary keyworded call to the
  list-param body.
- *Fold and pairwise have no prefix form — decided.* The prefix-list surface is
  unary-only; fold and pairwise are written solely as infix runs (`a + b - c`,
  `a < b < c`). So only the slot-led `OperatorChain` reaches the reducer for
  these modes — the head-keyword-over-list shape never needs routing into it.
- *Builtin operators stay as they are — decided.* `.`/`?`/`!` and the other
  compile-time operators keep their parse-time desugaring in
  [`operators.rs`](../../src/parse/operators.rs); they are out of scope for this
  mechanism, never appear as interior run keywords, and are not migrated onto
  it. The reducer operates only over registry-declared operators.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Anonymous structural unions](../type_language/anonymous-unions.md) — the `|`
  chaining surface rides this machinery.
- [User-defined operator modules](user-defined-operator-modules.md) — the
  declaration surface and `OP`/`GROUP` binders ride this mechanism.
