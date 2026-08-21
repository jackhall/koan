# Symbol-keyed scope binding tables

**Problem.** A scope's binding tables key by region-bumped text. `Bindings`
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs)) holds
`BumpBackedMap<'a, &'a str, _>` for its `data`, `types` and `operators` maps, and the
module-slot table in [src/machine/core/scope.rs](../../src/machine/core/scope.rs) does the
same. Everything a binding is keyed *by* elsewhere is now a
[`Symbol`](../../src/machine/model/labels.rs) — a signature's parameter schema, a record's
field index, a type digest's field feed — so the frame bind has to translate back across the
seam: `run_user_fn` ([src/machine/core/kfunction/exec.rs](../../src/machine/core/kfunction/exec.rs))
calls `render_label` once per parameter per call, allocating a `String` the table then keys a
bumped copy of. That resolve is the one place the label interner sits on a hot path rather
than a render path, and it is the only reason a call reaches the interner at all.

The text keys also cost a byte-wise compare and a string hash per lookup where a `Symbol`
compare is a `u128` equality, and they keep the scope walk's key currency different from the
dispatch schema's.

The label design this builds on is pinned in
[design/label-interning.md](../../design/label-interning.md).

**Acceptance criteria.**

- The scope binding tables — `Bindings`' `data`, `types` and `operators` maps and the
  module-slot table — key by `Symbol`, with no `&str` key type in any binding map.
- `bind_delivered_direct` and `register_type_direct` take a `Symbol`, and `run_user_fn`
  binds each parameter straight from the signature schema's symbol with no interner reach
  and no `String` built per parameter per call.
- A name lookup that arrives as source text (a scope walk from an `Identifier` part) computes
  `Symbol::of(text)` at the seam and compares symbol bits from there down.
- Diagnostics that name an unbound or shadowed binding resolve the text through the run's
  label interner, and a resolve miss renders the same stable placeholder the rest of the
  render paths use.
- The recorded per-call allocation count for a user-defined function with n parameters drops
  by the n `String`s the frame bind builds, measured by differencing an audit shape.

**Directions.**

- *Key type — decided per [design/label-interning.md](../../design/label-interning.md).*
  `Symbol`, identity-hashed, the same handle every other label site already carries.
- *Where the text→symbol conversion sits — open.* Either at the parse boundary (an
  `Identifier` part carries a `Symbol` beside its text, so the walk never converts) or at the
  lookup seam (`Symbol::of` per walk entry, cheap and local). Recommended: the lookup seam
  first, since it needs no AST change and `Symbol::of` is a pure hash.
- *Operator-group keys — open.* The operator table keys by an operator's token text, which is
  syntactic in the same sense a field name is; whether it shares the `Symbol` vocabulary or
  keeps its own key type is unresolved.

## Dependencies

**Requires:** none — `Symbol`, the run-frame label interner and the signature's symbol-keyed
parameter schema are shipped substrate ([design/label-interning.md](../../design/label-interning.md)).

**Unblocks:** none.
