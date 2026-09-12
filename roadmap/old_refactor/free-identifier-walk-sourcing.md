# Source the free-identifier walk's last two rules

The inferred-`CLOSE` walk reads its visibility rule and its label positions off the interpreter's
own definitions, so neither can drift from the resolver without a compile error.

**Problem.** `CLOSE (block)` infers its capture list with a structural walk over the block's raw
AST ([close_inference.rs](../../src/machine/model/close_inference.rs)). A walk that decides which
names a block binds is a second implementation of scoping, so the walk sources everything it can
from the readers the scheduler already drives off — `lazy_kinds_at`, `binder_plan`,
`announce_type_members`, `MACHINE_BINDERS`, the FN signature stride. Two rules have no definition
in the tree to source, so the walk states them itself and behavioral tests hold the copies to the
interpreter:

- **The positional visibility rule.** The interpreter's rule is `Scope::visible`
  ([bindings.rs](../../src/machine/core/bindings.rs)): a binding at statement index `idx` is visible
  to a reader at cutoff `c` iff `idx < c`, over runtime binding entries. The walk needs the same
  comparison over AST statement positions before any entry exists, and writes it again in
  `Walk::binds` — plus the three order-independent exceptions layered on top (a scope's seeded
  names, a module body's announcements, a nominal declaration's own binder window), whose *inputs*
  are sourced but whose "ignores the gate" status is restated. A change to the interpreter's
  cutoff does not break the walk's copy at compile time.
- **Label positions.** Which slots of a builtin form the body name-resolves and which it reads as a
  label is per-builtin body semantics with no table behind it: `identifier_sig`
  ([attr.rs](../../src/builtins/attr.rs)) types both `ATTR` slots `:Identifier`, and only the body
  says the lhs is a use and the field is a label. So `CLOSE_RULES` in the walk
  ([close_inference.rs](../../src/machine/model/close_inference.rs)) hand-lists `ATTR`'s
  field slot, `FROM`'s field list, record-literal keys, a pair run's name half and a union schema's
  tag half. Consistency tests assert the registration table holds a builtin under each
  special-cased form, which catches a rename or re-shape but not a builtin that changes whether it
  *resolves* a token it still accepts.

**Acceptance criteria.**

- One visibility predicate over `(declared position, reader position)` is defined once and called by
  both `Scope::visible`'s runtime lookup and the walk's scope stack; `Walk::binds` holds no
  comparison of its own.
- The order-independent windows (seeded names, module-body announcement, a nominal declaration's own
  binder) are expressed as a position value that predicate admits unconditionally, not as a
  separate branch in the walk.
- A builtin's registration declares, per slot, whether the body name-resolves the token, alongside
  the slot's type and its laziness ([forms.rs](../../src/parse/forms.rs)); the
  walk reads label-ness through that declaration and `CLOSE_RULES` carries no `Attribute` or
  `Projection` rule.
- A builtin whose body stops resolving a slot it still accepts, or starts resolving one it read as a
  label, fails to compile or fails a table-consistency test without a hand-written walk case
  changing.
- The behavioral inference suite — position-aware freeness, labels, self-recursion, module
  announcement — passes unchanged.

**Directions.**

- *Where the shared predicate lives — open.* Candidates: a free function beside `BindingIndex` in
  `bindings.rs` that takes two indices, or a method on a small `LexicalPosition` type both the
  runtime table and the walk's scope stack store. Recommended: the type — it is the "shared
  representation to key both off" the walk currently lacks, and it lets the block-wide window be a
  distinguished position rather than an `Option` branch.
- *How a slot declares resolution — open.* Candidates: a static keyed by `FormId` beside
  `CLOSE_RULES`, or a third field on a `FORMS` entry beside `lazy_slots` so one table answers
  "raw / label / use" per slot. Recommended: widen the `FORMS` entry — one table, one probe, one
  consistency test.
- *Structural label positions — decided.* Record-literal keys, a pair run's name half and a union
  schema's tag half are syntax shapes, not builtin slots, so they stay read structurally in the walk;
  only the keyed builtin forms move to the registration axis.

## Dependencies

**Requires:** none — a leaf refactor over shipped inference.

**Unblocks:** none tracked yet.
