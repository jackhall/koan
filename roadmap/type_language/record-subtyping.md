# Record projection

A `FROM` projection builtin that narrows a record value's type to disambiguate
incomparable dispatch arms.

**Problem.** koan has first-class structural records — a `KType::Record` type
(`:{x :Number, y :Str}`), an anonymous record value (`{x = 1, y = "a"}`), and
width/depth subtyping that orders record values in the dispatch lattice (see
[ktype.md § Variance](../../design/typing/ktype.md#variance)). But the lattice can
only order records that are width/depth comparable: two arms whose field sets
diverge — `:{x :Number, y :Str}` vs `:{x :Number, z :Str}` — are mutually
incomparable, so a value carrying all of `x`, `y`, `z` fills both and dispatch
surfaces `AmbiguousDispatch`
([resolve_dispatch.rs](../../src/machine/execute/dispatch/resolve_dispatch.rs)).
A caller has no way to narrow a record's carried type to select one arm.

**Impact.**

- A caller can narrow a record value's type at the call site to pick a specific
  dispatch arm when two arms are incomparable, resolving the ambiguity the lattice
  alone can't.
- The narrowing re-types rather than erases: the projection `Rc`-shares the backing
  record and narrows the carried field-type map — the same `stamp_type`
  ([kobject.rs](../../src/machine/model/values/kobject.rs)) move `List` / `Dict` /
  `Record` already make at an annotated boundary — so dropped fields stay physically
  present but invisible through the narrowed type, consistent with dispatch trusting
  the carried type rather than walking contents.

**Directions.**

- *Projection surface — open.* The narrowing builtin reads as `([x, y] FROM r)` —
  its first argument is a `List` of identifiers (the fields to keep). Surface keyword
  (`FROM`) and whether the identifier list is a literal-only position are open.
- *Projection is type-computing — decided.* Its result type is derived from the
  literal identifier list, so it routes like the dispatcher-only `_OF` ops
  ([scheduler.md](../../design/typing/scheduler.md)), not as an ordinary value
  builtin.
- *Projection semantics — decided: re-typing, not erasing.* Projection `Rc`-shares
  the backing record and narrows the carried field-type map via `stamp_type`. Dropped
  fields stay physically present but invisible through the narrowed type.

## Dependencies

**Requires:**

None — the standalone `KType::Record` type, the anonymous record value, and the
record width/depth subtyping this builds on have shipped (see
[ktype.md § Variance](../../design/typing/ktype.md#variance) and
[type-language-via-dispatch.md § Record-type sigil](../../design/typing/type-language-via-dispatch.md#record-type-sigil)).

**Unblocks:**

None — projection is a leaf disambiguation aid over the shipped record lattice.
