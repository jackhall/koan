# Tagged-union variants as dispatchable types

Converge tagged-union `MATCH` onto the ordinary type-dispatch that now eliminates
every other typed value — the deferred half of the variant work whose identity,
dispatch, and surface already [shipped](../../design/typing/user-types.md#tagged-union-variants).

**Problem.** A user-`UNION` value's `ktype()` now reports a
`KType::Variant { set, index, tag }`, so variants fill typed slots and dispatch
by identity — but `MATCH` (`src/builtins/match_case.rs`) is still a *distinct*
elimination form: it reads the value's carried tag and selects an arm by name
through the shared [`branch_walk`](../../src/builtins/branch_walk.rs), rather than
lowering to the ordinary type-dispatch that eliminates every other typed value.
Two consequences remain. A typo'd arm head (`MATCH (m) WITH (Bogus -> …)`) yields
the generic inexhaustive-match error, not a "Bogus is not a variant of Maybe"
schema error, because `branch_walk` doesn't carry the union's variant set. And a
field typed as a variant of the type currently being sealed has no `SetLocal`
form, so recursive variant references inside a schema are unsupported (external
`Variant`s pass through seal/resolve walks fine).

**Acceptance criteria.**

- `MATCH` lowers to ordinary type-dispatch — the same mechanism that eliminates
  [anonymous structural unions](anonymous-unions.md) by runtime type — rather
  than the parallel name-matching `branch_walk` form, so the two elimination
  paths converge.
- A `MATCH` arm whose head names a non-variant of the scrutinee's union is a
  schema error naming the offending tag and the real variants, not a generic
  inexhaustive-match miss.
- A schema field can be typed as a variant of the type currently being sealed:
  a `KType::Variant` inside a schema folds to its `SetLocal` form at seal and
  resolves back at projection, like `SetRef`.

**Directions.**

- *Variant identity as its own `KType` — decided, shipped.* Each declared variant
  is a `KType::Variant { set, index, tag }` refinement of its union, keyed
  structurally by `(union-set ptr, index, tag)` and reached *through* the union —
  namespace option (c), no global variant names. A variant is strictly more
  specific than its union's `SetRef` and than `AnyUserType { kind: Tagged }`;
  discrimination keys on `(set, index, tag)`, so same-payload variants stay
  distinct. See [user-types.md § Tagged-union variants](../../design/typing/user-types.md#tagged-union-variants).
- *Tag lexing / capitalization — decided, shipped.* Tags are capitalized `Type`
  tokens (`some`→`Some`, `ok`→`Ok`/`Error`), keyed by the tokenizer's
  capitalization rule via `FieldNameKind::Type` (`src/parse/triple_list.rs`); a
  lowercase tag is a clear parse error. A variant is therefore type-classified
  everywhere `classify_dispatch_shape` runs.
- *Variant-reference surface — decided, shipped.* A variant type is spelled
  `:(Maybe Some)` — a union-qualified sigil reaching the variant through its
  union, disambiguated from construction `(Maybe (Some v))` by the absence of a
  payload. No general `.` path operator. Variant-led construction (`(Some 42)`
  with the union inferred) stays deferred — it presumes a reachable bare variant
  name this namespace does not provide.
- *MATCH vs dispatch — deferred to [anonymous-unions](anonymous-unions.md).*
  `MATCH` shipped as a distinct fast-track form (option B): it selects the arm by
  the carried tag, now validated as a `Type` token, with O(1) variant admission
  on the dispatch fast path. Full lowering to type-dispatch / removing the
  parallel form rides the shared "match by type" sugar that item already defers.
- *Recursive variant references in a schema — open.* A field typed as a variant
  of the type currently being sealed needs a `SetLocal`-variant form in the
  seal/resolve walks. Recommended: add a `Variant` arm alongside `SetRef` in
  `seal_recursive_refs` / `resolve_set_locals`, mirroring the existing fold.

## Dependencies

Cross-link (not a dependency edge): [anonymous structural
unions](anonymous-unions.md) shares the type-dispatch elimination model — that item
handles *untagged* unions, this one supplies the variant `KType` so *tagged* unions
eliminate the same way — but neither blocks the other.

**Requires:**

- [Branch-arm return contract](../../design/execution-model.md#arms-as-own-blocks)
  — the `MATCH` arm machinery the remaining work lowers into type-dispatch.

**Unblocks:** none tracked yet.
