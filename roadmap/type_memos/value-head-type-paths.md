# Value-head type paths

Type-position paths rooted at a module project through the value channel; `KType` carries no
module variant.

**Problem.** A type-position path rooted at a module (`IntOrd.Type` as an annotation head, a
deferred `-> Er.Type` return) resolves through
[`KType::Module`](../../src/machine/model/types/ktype.rs) inside elaboration, and
`AbstractType` carries `&Module` (`AbstractSource::Module`) so further members can be
projected off it — the type channel still hosts a module variant and `KType` stays
lifetime-entangled with `Module`.

**Acceptance criteria.**

- Elaboration resolves a type path whose head is a module by projecting the type member off
  the value-channel module value.
- `KType` has no `Module` variant; `KKind::Module` is retired. (The `Module` surface
  keyword already lowers to the empty signature; this item removes the remaining
  type-position `KType::Module` arms and retires the now-unused `KKind::Module`.)
- `AbstractType`'s source is id-keyed and carries no `&Module`; further-member projection
  reads value-channel receivers.

**Directions.**

- *Phasing — decided.* Foundation phase (carries the risk; prototype first): value-head
  projection in elaboration — the `SigiledTypeExpr` deferral is the entry point; prototype
  the mechanics against the functor deferred-return tests before committing to an approach.
  Mechanical phases, each leaving the verify-koan slate green: compiler-guided deletion of
  `KType::Module` match arms, `KKind::Module` retirement, and swapping
  `AbstractSource::Module(&Module)` to an id-keyed source.

## Dependencies

**Requires:** none — its prerequisite (the `KObject::Module` value carrier) has shipped.

**Unblocks:**

- [Module naming flip](module-naming-flip.md) — snake_case heads in type position need
  value-head elaboration.
