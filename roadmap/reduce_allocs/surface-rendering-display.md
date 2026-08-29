# Surface rendering as a `Display` view

The `summarize` family renders every surface value to an owned `String`, so a diagnostic that
never prints and a `PRINT` that does pay the same nest of buffers.

**Problem.** Four rendering doors return `String` —
[`Held::summarize`](../../src/machine/model/values/carried.rs) and its `Carried` peer,
[`KObject::summarize`](../../src/machine/model/values/kobject.rs),
[`KType::name`](../../src/machine/model/types/ktype.rs), and the
[`Shape::summarize`](../../src/machine/model/ast/shape.rs) trait method that
[`ExpressionPart`](../../src/machine/model/ast.rs) and
[`WorkingPart`](../../src/machine/model/ast/working.rs) implement — and each composite arm
renders its children into a `Vec<String>` and `join`s it. A list of three renders four `String`s
and a `Vec`; a nested one multiplies. 73 production call sites reach the family, plus 65 in tests.

Almost every caller drops the `String` straight into a `format!`, where the text would have gone
into the message's own buffer had the door been a `Display` view. The interner already has that
shape beside its owned door: [`LabelInterner::display`](../../src/machine/model/labels.rs) hands
back a `LabelDisplay` that writes the recorded text into the caller's formatter, and
`display_label` / `render_label` in [ktype.rs](../../src/machine/model/types/ktype.rs) are the
same pair one tier up. The `summarize` family has no such peer, so a caller that wants the text in
a formatter has no route but the owned one.

Two callers genuinely need the bytes rather than a formatter, and both pay twice for them.
[`PRINT`](../../src/builtins/print.rs) renders to a `String` and then copies it into the step's
region, where a bump-hosted render would have landed the text once and returned the same carrier —
and `PRINT` returns the rendered string, so producing it is not optional.
[`KKey`](../../src/machine/model/values/kkey.rs) renders to key a dict.

**Acceptance criteria.**

- Every arm of the surface-rendering family — `Held`, `Carried`, `KObject`, `KType`'s name, and
  the `Shape` trait's part rendering — is reachable as a `Display` view that writes into the
  caller's formatter without an owned `String`.
- No composite arm collects its children into a `Vec<String>` to `join` them.
- A diagnostic that names a value builds one buffer: its own message.
- `PRINT` renders its value once, into the region text it returns, with no heap `String` on the
  way.
- The `wide_step` term falls against the recorded sweep, and
  `tests/allocation_baseline.rs` is rebaselined to the new figures with its headroom intact.

**Directions.**

- *View shape — open.* A `Display`-implementing view struct per door (`value.summary(registries)`
  returning a `Summary<'_>`, mirroring `LabelInterner::display`), or a
  `write_summary(&self, formatter, registries)` method each door's `Display` impl delegates to.
  The trait method on [`Shape`](../../src/machine/model/ast/shape.rs) constrains the choice: a
  view struct borrowing `&self` through a trait needs an associated type or a boxed return, while
  a `write_summary` signature stays object-safe. Recommended: `write_summary` at the trait, view
  structs at the inherent doors that compose over it.
- *Owned door — open.* Whether `summarize` survives as a thin
  `self.summary(registries).to_string()` for the callers that want ownership (the 65 test
  assertions, `KKey`), or every caller converts. Recommended: keep it, so the conversion is
  additive and the test tree does not move.
- *`PRINT`'s region render — decided.* Render into a `bumpalo` string over the step's destination
  region through the new view, so the bytes written out and the bytes the returned `KString`
  carries are the same allocation.
- *Sequencing — decided.* Leaves first (`KType::name`, `KObject`'s scalar arms), then the
  composite arms that currently `join`, then `Held` / `Carried`, then `PRINT`. One
  `python3 tools/alloc_audit.py` between each.

## Dependencies

**Requires:** none — every door is local to its own type.

**Unblocks:** none tracked yet.
