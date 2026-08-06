# Drop-free `KFunction`

Moves the function family out of its typed cell into the region bump — the
storage model of
[design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state).

**Problem.** A `KFunction`'s only owned heap is its signature's
`elements: Vec<SignatureElement>`, whose keyword and argument names are
`String`s ([src/machine/model/types/signature.rs](../../src/machine/model/types/signature.rs)).
That one vector is what holds the whole family in a typed cell that must run
`Drop` at region death; the other fields (`Body`, `ReturnType`, the captured
scope reference, the interned `KType`) are already `Copy`.

**Acceptance criteria.**

- `ExpressionSignature` stores its elements as a bumped slice with bumped
  `&str` names, allocated at the signature's own region — builtin registration
  mints into run-root, `FN` into the captured scope's region.
- `KFunction` is `Drop`-free and stored through the bump doors; its typed cell
  and `Stored` impl are deleted.
- The Miri leak slate stays clean over function-definition-heavy programs.

## Dependencies

**Requires:**


**Unblocks:**

- [Frame-owned scopes retire the typed cells](frame-owned-scopes.md)
