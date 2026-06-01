# Argument-binding unification

Bind a call's resolved arguments into the callee scope as one record block
under a single frame-level binding index, instead of per-entry.

**Problem.** Installing a call's arguments into the callee scope rebuilds a
map entry-by-entry across three representations. Dispatch resolves arguments
into
[`ArgumentBundle { args: HashMap<String, Rc<KObject>> }`](../../src/machine/core/kfunction/argument_bundle.rs),
then the invoke path installs them into
[`Bindings.data: HashMap<String, (&KObject, BindingIndex)>`](../../src/machine/core/bindings.rs),
tagging every entry with a `BindingIndex`. But `BindingIndex` is the *lexical
position of the installing statement* — and all of a call's parameters
install at the same position, so the per-entry `(value, index)` pairing
stores one index redundantly across the whole parameter block.

**Impact.**

- A call's arguments install as one record block under a single frame-level
  binding index, so the per-entry index tagging disappears from the
  parameter-bind path.
- The resolved-argument carrier and the scope's value map share the
  [record substrate](record-substrate.md) shape, so binding is an
  extend/move of the argument record rather than an entry-by-entry copy into
  a differently-shaped container.
- A field's binding-index lookup, where still needed, derives from its
  position in the ordered record instead of a stored per-entry pair.

**Directions.**

- *Frame-level index — decided.* Carry one `BindingIndex` per parameter
  block at the frame, not one per entry. The visibility predicate
  (`Bindings::visible`) reads the frame index.
- *Shared carrier — open.* Whether `Bindings.data`, `ArgumentBundle`, and
  `Struct.fields` collapse to one Rust container, or keep distinct ownership
  (arena-ref vs. `Rc` vs. owned) over a shared shape, is the open
  implementation choice. *Recommended: one shape, value-type parameter; keep
  ownership distinct.*
- *Invoke-path rewrite — deferred.* The exact rewrite of `KFunction::bind` /
  invoke to emit a record block is sequenced after the substrate lands.

## Dependencies

**Requires:**

- [Record substrate for identifier-keyed binding](record-substrate.md) — the
  shared shape the argument record and scope map both adopt.

**Unblocks:**

None — this is the runtime-efficiency payoff of the substrate. (Soft, not a
dependency edge: per-call type-parameter binding's invoke-path wiring rebases
onto this block-install path if this lands first, but isn't blocked by it.)
