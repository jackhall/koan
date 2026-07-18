# Signature types own their schema

Collapse `SigSource` so a signature type is owned data — schema, sig-id, diagnostic
path — instead of a region pointer into the declaring module or signature value.
Part of the arc landing
[design/typing/type-registry.md](../../design/typing/type-registry.md).

**Problem.** `KType::Signature` carries a `SigSource<'a>` —
`Declared(&'a ModuleSignature)` / `SelfOf(&'a Module)` / `Empty`
([`ktype.rs`](../../src/machine/model/types/ktype.rs)) — the only region pointers a
`KType` can hold and the sole reason `KType<'a>` has a lifetime parameter (every
other parameterized type in the layer is `'a` only transitively, through the
`KType`s its schemas embed). The residence audit
([`residence.rs`](../../src/machine/core/arena/residence.rs)) exists on the type
side to police those pointers' region confinement. ATTR over a first-class
signature value resolves member and `VAL`-slot lookups by reverse-looking-up the
declaration scope ([`attr.rs`](../../src/builtins/attr.rs), `access_type_member`)
even though `ModuleSignature` already projects an owned schema at construction.

**Acceptance criteria.**

- `KType::Signature` owns its content — schema, `ScopeId` sig-id, diagnostic path
  string, pinned slots — and `SigSource` no longer exists; no `KType` variant
  borrows region data.
- The empty signature, a `SIG`-declared interface, and a module's self-sig are one
  kind of owned-schema signature type; the same-module fast path is digest
  equality on the schema digests.
- ATTR over a first-class signature value answers member and `VAL`-slot lookups
  from the owned schema; no decl-scope reverse-lookup remains on that path.
- Type digest values are unchanged — the existing digest test suite passes
  unmodified.

**Directions.**

- *Interim content transport — decided.* The variant holds `Rc<SigSchema>` until
  interning dedups it (the same role `Rc<RecursiveSet>` plays for nominal sets);
  [Interned type content behind Copy handles](interned-type-content.md) unwraps the
  `Rc` into a registry node.
- *Self-sig at creation — decided.* A module builds and caches its owned self-sig
  schema at creation (it already caches the digest), so extracting a module value's
  principal signature type stays context-free.

## Dependencies

**Requires:** none — the schema projection and content digests it builds on are
shipped.

**Unblocks:**

- [KType without a lifetime parameter](lifetime-free-ktype.md) — with the region
  pointers gone, the lifetime parameter is vestigial and can be deleted.
