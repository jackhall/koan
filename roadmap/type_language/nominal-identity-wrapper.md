# Collapse `UserTypeKind` into a nominal-identity wrapper

Replace the four-kind `UserType { kind, scope_id, name }` enum with a single
nominal-identity tag over an existing structural-repr `KType`, deleting
`UserTypeKind` and the duplicate schema payloads it carries.

**Problem.** `KType::UserType { kind: UserTypeKind, scope_id, name }`
([`src/machine/model/types/ktype.rs`](../../src/machine/model/types/ktype.rs))
groups four nominal user-type kinds — `Struct { fields: Rc<Record<KType>> }`,
`Tagged { schema: Rc<HashMap<String, KType>> }`, `Newtype { repr: Box<KType> }`,
`TypeConstructor { schema, param_names }` — behind a `UserTypeKind` enum, with a
parallel `AnyUserType { kind }` wildcard and a hand-written tag-only `Eq` / `Hash`.
Each kind's schema payload restates a shape `KType` already carries (or soon
will): `Struct`'s `fields` is the same `Record<KType>` the structural
`KType::Record` carries (ktype.rs:143); `Newtype` is bare nominal identity over a
`repr` it excludes from equality; `Tagged`'s schema is a disjunction — the
structural union `KType` that [anonymous structural
unions](anonymous-unions.md) introduces; `TypeConstructor` pairs with
`KType::ConstructorApply`. The only residue unique to `UserType` is the nominal
`(scope_id, name)` identity — itself a pattern `KType::AbstractType { source,
name }` already implements (ktype.rs:221). The layer is thus a parallel encoding
of "nominal identity + structural repr," its repr half duplicating other `KType`
variants.

**Impact.**

- *One nominal-identity wrapper, no kind enum.* A single `KType` variant carries
  `(scope_id, name)` over a structural-repr `KType`; `UserTypeKind`, its sentinel
  constructors, and its tag-only `Eq` / `Hash` are deleted.
- *Each schema shape lives in one place.* A nominal struct reuses
  `KType::Record`'s record machinery instead of a parallel `Struct { fields }`
  payload; a nominal tagged type reuses the union repr; a newtype carries only its
  wrapped type.
- *Nominal identity is a single notion.* The `(scope_id, name)` identity the
  wrapper shares with `AbstractType` is expressed once rather than re-encoded per
  user-type kind.

**Directions.**

- *Nominal-identity wrapper — decided.* Replace `UserType { kind, scope_id, name }`
  with one `KType` variant carrying `(scope_id, name)` and a structural-repr
  `KType`. Dispatch and equality key on `(scope_id, name)` only — repr ignored —
  preserving today's payload-ignored equality and the SCC cycle-close sentinel
  behaviour (the sentinel becomes an empty repr).
- *Per-kind repr mapping — decided.* Struct → `KType::Record`; Newtype → the
  wrapped `KType`; Tagged → the structural union repr from [anonymous structural
  unions](anonymous-unions.md).
- *`TypeConstructor` — open.* Higher-kinded constructors entangle with
  `KType::ConstructorApply` / `Mu` and carry `param_names`; whether they fold into
  the wrapper or stay a distinct variant is unsettled. Recommended: fold the three
  first-order kinds, and keep `TypeConstructor` separate if its HKT repr resists
  the wrapper — accepting that may leave one residual variant after `UserTypeKind`
  is deleted.
- *`AnyUserType` wildcard — open.* `:Struct` / `:Tagged` would admit by the
  wrapped repr's shape (Record vs union) rather than a `kind` discriminant.
  Recommended: a repr-shape predicate, verified to stay as cheap as today's
  tag match.
- *Merge with `AbstractType` — deferred.* Whether the nominal-identity wrapper and
  `KType::AbstractType` collapse into one shared nominal-identity carrier is a
  further step, out of scope here.

## Dependencies

**Requires:**

- [Anonymous structural unions](anonymous-unions.md) — supplies the structural
  union `KType` that a nominal tagged type wraps as its repr; without it `Tagged`
  has no existing variant to fold into and `UserTypeKind` cannot be fully removed.

**Unblocks:** none tracked yet.

Capstone over the union-representation work: it also pairs with the tagged-union
variants-as-types item (both reshape how a `UNION` is represented), though it does
not hard-require it. The naive alternative — flattening to four sibling `KType`
variants (`Struct` / `Tagged` / `Newtype` / `TypeConstructor`) — is rejected: it
multiplies the wildcard and specificity dispatch arms fourfold, since the current
grouping already consolidates them. The win is reusing existing structural reprs,
not expanding the variant set.
