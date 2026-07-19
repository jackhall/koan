# Abstract members carry arity

Retire the sentinel `TypeConstructor` sets that stand in for a SIG's abstract
higher-kinded members; `KType::AbstractType` gains an arity and covers both orders.

**Problem.** A SIG's first-order abstract member (`TYPE Elt`) is a
`KType::AbstractType`, but its higher-kinded sibling (`TYPE (Type AS Wrap)`) is a
singleton `RecursiveSet` member of kind `TypeConstructor` minted with
`ScopeId::SENTINEL` ([`type_decl.rs`](../../src/builtins/type_decl.rs),
`mint_type_constructor`). The sentinel scope id is the only thing distinguishing that
mint from a real `NEWTYPE (Type AS Wrap)`: the set content — member name, kind, empty
schema, parameter names — is byte-identical, and `scope_id` is excluded from the set
digest, so the two digest equal while the read sites classify by `scope_id ==
SENTINEL` off the specific `Rc` allocation (four sites in
[`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs), `schema_self_ref` in
[`type_digest.rs`](../../src/machine/model/types/type_digest.rs), the generative-mint
trigger in [`ascribe.rs`](../../src/builtins/ascribe.rs), and the abstract-slot guard
in [`apply_callable.rs`](../../src/machine/execute/dispatch/apply_callable.rs)). Two
digest-equal types therefore behave differently depending on which allocation carries
them, contradicting digest-equality-is-type-identity
([type-identity.md](../../design/typing/type-identity.md)) — and under interned
content the two would collapse to one node whose classification depends on intern
order.

**Acceptance criteria.**

- `KType::AbstractType` carries `arity: usize` — `0` is a proper type, `n > 0` a type
  constructor taking `n` parameters — and a SIG's higher-kinded abstract member is
  represented as an `AbstractType`, never as a set member.
- No site classifies a type by a set member's `scope_id`: the sentinel reads listed
  in the Problem match on `AbstractType`, and `NominalMember.scope_id` is
  diagnostics-only.
- `kind_of` classifies an `AbstractType` with `arity > 0` as
  `KKind::TypeConstructor` — so a `ConstructorApply` over an abstract constructor
  classifies into the right family — and `arity == 0` as `ProperType`;
  `constructor_arity` reads the field without projecting a schema.
- `AbstractType` digest identity remains `(source, name)` with arity excluded, so
  every first-order abstract-type digest (opaque-ascription mints, slot tags) is
  byte-unchanged, and `schema_content_digest`'s arity bytes keep today's encoding
  (`0 → 0x00`; `n → 0x01` + count); the digest suite's pinned relations hold.
- Opaque ascription's generative mint triggers on a higher-kinded `AbstractType`
  member exactly where it triggered on a sentinel set member;
  [`tests/functor_e2e.rs`](../../tests/functor_e2e.rs) and the ascribe suites pass
  unchanged.
- The full test slate is green.

**Directions.**

- *Arity on the variant, excluded from the digest — decided.* The same
  `(source, name)` can never bind two arities, so the field is derivable payload;
  excluding it keeps every existing first-order `AbstractType` digest stable.
- *KError's synthetic singletons keep `ScopeId::SENTINEL` — decided.* They are
  `NewType`-kind; once the constructor reads die, no site keys on their scope id.

## Dependencies

**Requires:** none — `AbstractType` and the sentinel mint are shipped surface.

**Unblocks:**

- [Interned type content behind Copy handles](interned-type-content.md) — one node
  per digest makes allocation-carried sentinel classification unsound; this item
  removes it first.
