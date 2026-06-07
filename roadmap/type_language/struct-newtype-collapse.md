# Record-repr NEWTYPE recursion

The product-side `STRUCT` → record-repr `NEWTYPE` collapse shipped — a struct is a
`NominalKind::Newtype` over a `KType::Record`, carried as `Wrapped`, with the `STRUCT`
declarator and `KObject::Struct` carrier retired. The one piece left is recursion through a
record repr: `NEWTYPE Node = :{value :Number, next :Node}` and record newtypes co-declared
in a `RECURSIVE TYPES` block.

**Problem.** The `NEWTYPE` declarator
([`newtype_def.rs`](../../src/builtins/newtype_def.rs)) fills its repr without threading its
binder name or sealing it — it resolves the repr slot to a `KType` and calls
`member.fill(NominalSchema::Newtype(repr))` directly. A non-recursive record sigil
`:{x :Number, y :Number}` resolves fine (the `:{…}` sub-dispatches through the `RECORD`
builtin in [`type_constructors.rs`](../../src/builtins/type_constructors.rs) to a
`KType::Record`), but a self-reference `:{next :Node}` resolves `Node` mid-declaration and
misses: nothing lowers a transient `KType::RecursiveRef`, so nothing seals to a `SetLocal`.
The retired `STRUCT` declarator threaded its name through
`parse_typed_field_list_via_elaborator` and sealed with `seal_recursive_refs` /
`finalize_nominal_member`; that path is gone. Three `RECURSIVE TYPES` block tests are
`#[ignore]`d on this gap
([`recursive_types/tests.rs`](../../src/builtins/recursive_types/tests.rs)).

**Acceptance criteria.**

- `NEWTYPE Node = :{value :Number, next :Node}` seals its `next` back-edge to a `SetLocal`
  into the declaring member's singleton set, and projects + navigates the same as a
  self-recursive struct did.
- Record newtypes co-declared in a `RECURSIVE TYPES` block seal into one shared set with
  cross-references as `SetLocal` indices — the three `#[ignore]`d `recursive_types` block
  tests pass unmodified.
- A `:(LIST OF Node)` field inside a record repr threads its `Node` reference too (the shape
  the migrated `Tree` recursion test exercises).

**Directions.**

- *Raw-capture the `:{…}` repr — decided.* The declarator must own the field-list
  elaboration to thread its binder name, so it captures the record sigil *raw* (a
  `:SigiledTypeExpr` slot, the way the FN return overload captures `:(…)`) and runs
  `parse_typed_field_list_via_elaborator` with `Elaborator::with_threaded([name])`, then
  seals via `finalize_nominal_member` / `seal_recursive_refs` into
  `NominalSchema::Newtype(KType::Record(sealed))`. Scalar/leaf reprs keep the current
  resolved-`TypeExprRef` path.
- *Block membership — decided.* Routing the seal through `finalize_nominal_member` also makes
  a `NEWTYPE` member of a `RECURSIVE TYPES` block fill the block's shared set (the
  pre-installed `SetRef`) instead of minting a singleton.
- *Deferred sibling refs — open.* A repr field naming a still-finalizing sibling needs the
  re-elaboration the struct path did with `defer_struct_via_combine`; reuse
  `type_constructors`' `defer_via_combine` shape. Recommended: mirror it.

## Dependencies

Assumes the same-scheduler value-side-placeholder bug is resolved (a record-repr `NEWTYPE`
declared alongside a dependent `NEWTYPE` in one scheduler leaks a stale placeholder — see
`scratch/scheduler-stale-placeholder-bug.md`), since a `RECURSIVE TYPES` block declares
several record newtypes in one scheduler.

**Requires:**

- [Type-only nominal identities](../../design/typing/user-types.md) — the shipped
  `NominalKind::Newtype` / `Wrapped` substrate and the `:{…}` → `KType::Record` resolution
  this threads recursion through.

**Unblocks:**

- [Tagged-union variants as dispatchable types](tagged-variant-types.md) — a recursive
  tagged union threads a `SetLocal` back-edge through the union name, the same nominal-anchor
  threading this establishes for record-repr newtypes.
