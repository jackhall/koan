# SIG keyworded surface

Let a signature declare a module's keyworded (dispatch-bucket) members, so
the abstraction barrier can govern them.

**Problem.** A module's callable surface has two halves, and a SIG can name
only one. A `VAL <name> :<FnType>` slot governs a function *value* member
called through the value lane (`m.pure {x = 3}`), but a member defined `LET
pure = FN (PURE x :Number) …` also registers `PURE` in the module's dispatch
buckets, and opaque ascription replays those buckets verbatim
([`bulk_install_from`](../../src/machine/core/bindings.rs) — "the view
preserves the source module's keyworded surface as-is"). Inside `USING <view>
SCOPE`, calling `PURE 3` therefore reaches the source function directly:
arguments and results cross in the source module's types, bypassing the
boundary coercion the view applies to the same function read as a value slot.
There is no SIG syntax to declare a keyworded member, so satisfaction cannot
check the keyworded surface and ascription has no declared type to coerce it
against.

**Acceptance criteria.**

- A SIG can declare a keyworded member (surface syntax to be designed),
  including its slot types and return type, over the signature's abstract
  members.
- Satisfaction checks a module's dispatch-bucket entries against the SIG's
  declared keyworded members.
- A keyworded member called through an opaque view — `USING` window included —
  coerces at the boundary exactly as a function-typed VAL slot does: results
  carry the view's types, view-typed arguments reach the underlying function
  in the source's types.
- A keyworded member the SIG does not declare is absent from the opaque
  view's buckets (or its retention is an explicit, documented decision).

**Directions.**

- *Declaration syntax — open.* Something shaped like the FN definition head
  (`SIG S = ((OP (PURE x :Number) -> :(Number AS Wrap)))`) so the bucket key
  derives the same way; alternatives welcome at design time.
- *Coercion mechanism — decided.* Reuse the shipped barrier machinery — the
  [`coerce_object_into`](../../src/machine/model/values/coerce.rs) walk and the
  `Body::CoercedDelegate` eta-wrapper
  ([design/typing/modules.md](../../design/typing/modules.md)): a declared
  keyworded member's replayed bucket entry wraps the underlying function the
  same way a coerced VAL slot does.

## Dependencies

**Requires:** none — the coercion walk and the delegate body it reuses have
shipped.

**Unblocks:** none tracked yet.
