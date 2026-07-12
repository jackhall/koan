# KObject module carrier

Module values ride the value channel's Object arm: `KObject::Module`, typed by the
self-sig.

**Problem.** A module value rides the value channel's `Type` arm as
[`KType::Module`](../../src/machine/model/types/ktype.rs) — a type that types nothing: it is
rejected as a resolved return type, and slot admission goes through signatures. Because the
real module checks live on the type-channel side, the dispatch predicates carry compensating
arms — `matches_value` answers false for every `Signature` slot, and its `OfKind` arm
special-cases modules out
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)) — and
[`resolve_type_identifier`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
bridges module identities into the value channel for ATTR receivers.

**Acceptance criteria.**

- `KObject::Module` exists; a module named in expression position surfaces as an Object-arm
  value whose `ktype()` reports the principal signature.
- `matches_value` admits a module into a signature slot via the self-sig subtype check; the
  compensating arms are gone — `matches_value` handles `Signature` slots directly and
  `OfKind` no longer special-cases modules.
- ATTR member access, `USING … SCOPE`, `:!`/`:|`, and functor application operate on the
  Object-arm carrier.
- The value channel never carries `KType::Module`; the variant appears only in type-position
  elaboration paths.

**Directions.**

- *Phasing — decided.* Foundation phase (carries the risk): the carrier variant with its
  region/lift/witness plumbing, `ktype()` reporting the self-sig, and
  `resolve_type_identifier` surfacing a module as the Object-arm value rather than a
  `KType::Module`. Mechanical phases, each leaving the verify-koan slate green: removal of
  the compensating predicate arms, then a call-site sweep across ATTR, `USING`, ascription,
  and functor application.

## Dependencies

**Requires:** none — the structural admission rule its value-side check reuses has shipped
([Structures and signatures](../../design/typing/modules.md#structures-and-signatures)).

**Unblocks:**

- [Value-head type paths](value-head-type-paths.md) — elaboration projects through the
  Object-arm carrier.
