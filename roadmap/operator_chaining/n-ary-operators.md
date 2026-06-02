# User-definable n-ary operators

The fold pre-pass that lets a recognized operator chain evaluate — decomposing
a run of declared operators into nested binary sub-dispatches by precedence.

**Problem.** An operator chain is recognized but cannot yet *evaluate*. A
slot-led run of two or more operators (`a + b + c`, `A | B | C`) parses to the
cached `OperatorChain` shape (see
[expressions and parsing](../../design/expressions-and-parsing.md)), and its
operator group resolves through the per-scope operator registry walked by
[`resolve_operator_group_with_chain`](../../src/machine/core/scope.rs) (see
[the lookup protocol](../../design/typing/lookup-protocol.md)). But the
`OperatorChain` dispatch arm in
[`operator_chain.rs`](../../src/machine/execute/dispatch/operator_chain.rs)
terminates at an explicit fold seam — a `"operator-chain folding not yet
implemented"` error — because there is no bucket for a chain's long key and
nothing yet decomposes it into the binary sub-dispatches the binary bucket can
serve.

**Impact.**

- *Operators chain to arbitrary arity.* A run of one operator folds into a
  single evaluated form, each step resolved against the binary bucket the `OP`
  binder already populates.
- *Precedence and associativity drive grouping.* Mixed-operator chains within
  one group (`a + b * c`) group by each operator's declared tier, so a family
  declared together reads as written.
- *The disjunction surface evaluates.* The `|` surface of
  [anonymous structural unions](../type_language/anonymous-unions.md) ceases to
  be a recognized-but-inert chain and produces a union value.

**Directions.**

- *Fold pre-pass — decided.* A chain of two or more operators has no bucket for
  its long key, so a fold pre-pass decomposes it into nested binary
  sub-dispatches, each hitting the binary bucket with normal specificity. It
  reads each operator's tier and associativity off the resolved
  [`OperatorGroup`](../../src/machine/model/operators.rs) (the
  `member_operators` / `entry` surface the registry already exposes).
- *Precedence climb — decided.* The fold resolves grouping with a precedence
  climb over the flat operator key, reading each operator's tier and
  associativity from its group. Operators that may mix in one chain are declared
  together in one group, fixing their relative precedence; **chaining operators
  across groups stays disallowed**, falling out as the cross-group registry miss
  rather than guessing.
- *Variadic operators — deferred.* True variadic operators — one implementation
  invoked over a whole run — are deferred as an optimization for cases wanting
  flatness (an `A | B | C` union built once rather than spliced, which folding
  does in O(N²)).
- *Builtin-operator migration — deferred.* `.`/`?`/`!` keep their parse-time
  desugaring in [`operators.rs`](../../src/parse/operators.rs) and never appear
  as interior chain keywords. Whether they ever move onto this mechanism
  (gaining precedence tiers) is a later call; until then the fold operates only
  over registry-declared operators.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Anonymous structural unions](../type_language/anonymous-unions.md) — the `|`
  chaining surface rides this machinery.
- [User-defined operator modules](user-defined-operator-modules.md) — the
  declaration surface and `OP` binder ride this mechanism.
