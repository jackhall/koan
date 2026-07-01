# Object read-site carrier

**Problem.** A value read back by name or ATTR member re-wraps a bare reference.
[`Scope::resident_object_carrier`](../../src/machine/core/scope.rs) wraps an already-built
`&'a KObject` as `Witnessed::new(.., FrameSet::singleton(home))` — six callers: FN def
([`fn_def/finalize.rs`](../../src/builtins/fn_def/finalize.rs)), name resolution
([`scope.rs`](../../src/machine/core/scope.rs)), ATTR member access
([`attr.rs`](../../src/builtins/attr.rs), two sites), the LET object RHS
([`let_binding.rs`](../../src/builtins/let_binding.rs)), and the literal Resolved arm
([`dispatch/literal.rs`](../../src/machine/execute/dispatch/literal.rs)). The re-wrap exists because
[`Bindings`](../../src/machine/core/bindings.rs) stores `data` as `(&'a KObject, BindingIndex)` and
`types` as `(&'a KType, BindingIndex)` — no carrier, so a lookup has no reach to return and asserts a
single-frame one instead. The FN-def / LET-RHS **define** sites likewise hand out a bare `&'a` and
re-wrap it into a carrier downstream, so the registered reference and the carrier come from two
separate steps.

**Acceptance criteria.**

- `Bindings` carries each value binding's reach (its `FrameSet`, or its `Sealed` carrier); a name or
  ATTR lookup returns that witnessed carrier, and no lookup re-wraps a bare `&KObject`. This realizes the
  bind-site reach-set of
  [§Storage and access](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into) and
  retires the transitional `resident_object_carrier` named in
  [§Construction](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node).
- A freshly-built FN-def / LET-object-RHS object is born witnessed: it allocates through its scope and
  seals under that scope's frame in one call, so the registered `&'a` and the returned carrier share one
  allocation and co-location is structural — the FN def that *yokes* its `KObject::KFunction` onto a
  carrier witnessed by the defining scope's frame in
  [§Construction](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node).
- `Scope::resident_object_carrier`'s asserted `Witnessed::new` no longer exists; the object-read callers
  route the retained carrier instead.

**Directions.**

- *Object read-site carrier — decided.* Store each value binding's reach on the classified `Bindings`
  entry — the value/type split that hangs it is now structural (a value bind and a type bind of one name
  are mutually exclusive by construction), so a name / ATTR lookup returns a witnessed carrier instead of
  re-wrapping a bare `&KObject`.
- *Object define-site — decided.* Move alloc + seal into one `Scope` method (built on the foundation's
  `alloc_witnessed` + `into_set`), so a freshly-built FN / LET-RHS object is witnessed by the scope that
  allocated it; the bare `&'a` needed for registration and the carrier returned to the scheduler come
  from the same call.

## Dependencies

**Requires:**

- [The honest single-region witness substrate](../../src/witnessed.rs) — the define-site alloc + seal builds on the honest `yoke` witness and its `into_set` lift.

**Unblocks:**

- [Witnessed type and region operands](type-operand-carriers.md) — the capstone's `Witnessed::new` deletion needs this item's object-read callers retired.
