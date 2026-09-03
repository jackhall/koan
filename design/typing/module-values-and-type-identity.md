# Module values and type identity

Koan's module representation and type identity, in five facets that form one picture:
modules are values typed by signatures; satisfaction is structural subtyping against a
creation-time principal signature; `:Module` is the empty signature; type identity is a
content-hash digest; subtype outcomes are memoized on that digest. [modules.md](modules.md)
and [ktype/README.md](ktype/README.md) carry the mechanism each facet rests on.

## Modules are values

A module is a runtime value — `KObject::Module` — never a type. Nothing is typed *by* a
module, so `KType` carries no module variant: the type channel contains only things that can
type a field. Module names are snake_case value identifiers (`int_ord`); `MODULE` binds
value-side. Signature names use the Type-token spelling with no suffix (`Ordered`), making
the Type-token namespace exactly the set of things that type fields. Member access is ATTR
over the value. A type expression whose head names a module — e.g. a return type `er.Carrier`,
where `er` is a module-valued parameter — resolves by reading the named type member off the
module value. A concrete module's identity is never a
slot or return type — slots and returns name signatures, and a module's own signature is
named `:(TYPE OF int_ord)` (see
[modules.md § Modules in type position](modules.md#modules-in-type-position-type-of)).

## Signatures are the types of modules

A signature is a structural type over module shapes, with a canonical subtyping relation
modeled on record width/depth subtyping: `Sub <: Super` iff `Sub` supplies every `Super`
member — manifest type members equal, abstract type members unconstrained, VAL slots
type-compatible. SIG bodies distinguish **abstract** members (no concrete type given; a
client may pin one with `WITH`) from **manifest** members (pinned to a concrete type; a
conflicting `WITH` pin is a type error).

Every module derives its **principal signature** (self-sig) once, at creation, from its
body — a signature listing every member the module contains, each type member pinned to its
concrete definition. `ktype()` of a module value reports the self-sig, so dispatch trusts
the carried type: a `:Ordered` slot admits a module iff `self_sig <: Ordered`. Satisfaction
is structural — no ascription is required for admission — and a module's type never changes
after creation. Implicit
resolution keeps its lexically-scoped candidate set ([implicits.md](implicits.md)), so
structural satisfaction does not widen implicit search.

A signature's type identity is its schema content: a `SIG` digests over its members
(names, each abstract member's parameter names, manifest and VAL-slot types, with
references to the
signature's own abstract members canonicalized by name), and a module's principal
signature digests over its sealed self-sig. Two textually identical `SIG`
declarations, and two unascribed modules with identical interfaces, are one type —
the same structural-unification rule that governs every other `KType`.

Ascription operators construct views; they do not grant admission: `:!` asserts satisfaction and yields a
transparent view; `:|` is generative, minting fresh abstract-type identities per
application. `AbstractType` identity is id-keyed — `KType` holds no `&Module` — so
the abstract-type slots a `:|` view mints flow into its self-sig's content digest
unchanged, keeping two opaque views distinct.

## The empty signature

The empty signature is the top of the module lattice: every module's self-sig is a subtype
of it. The `Module` surface keyword lowers to it, so "any module" slots (`USING`'s receiver)
are signature-typed like every other module slot — the module/signature story has no
kind-wildcard exception.

## Joining two signatures

Two signature types have a least upper bound under the width/depth relation above, so a
container of modules with differing self-sigs memoizes their least common interface rather
than `Any` — a list of modules that each satisfy `Ordered` fills a `:(LIST OF Ordered)` slot
([`join_schemas`](../../src/machine/model/types/sig_schema.rs), the `Signature` arm of
`TypeRegistry::join`).

- **Width intersects.** A member only one operand names is dropped: the bound may promise
  only what both operands supply. Two signatures sharing no members join to the empty
  signature — the module-lattice top `:Module` — never to `Any`.
- **Depth reconciles per member.** Two equal manifest bindings survive manifest. Anything
  else at a matching kind — two differing manifests, a manifest against an abstract, two
  abstracts — demotes to an *abstract* member at that kind, the strongest requirement both
  bindings still satisfy. A kind disagreement (one side first-order, or two constructors over
  different parameter names) has no common requirement, so the member drops.
- **Value slots join pointwise, through the demoted members first.** A slot typed by one
  operand's binding of a demoted member and the other's rejoins as a reference to that
  member, rather than coarsening. The generalization is variance-aware: a function slot's
  return joins covariantly while its parameters *meet*, so widening a parameter never claims
  a satisfying module accepts arguments neither operand does.

A joined schema carries the canonical `ScopeId::SENTINEL` binder every projected SIG carries
and mints nonce-free abstract members, and the schema digest ignores `sig_id` — so a joined
signature is content-identical to the equivalent written `SIG` declaration and interns to the
same handle.

## Content-addressed type identity

Type identity is a wide content-hash digest, computed eagerly bottom-up when the type is
created, from the type's content alone — no raw-pointer identity in `KType`, no dependence
on interning order, no shared interner. The digest is wide enough that equality is one
digest compare with no repair path; opaque ascription stays generative by minting a
per-application nonce into the digested content. The full design is
[type-identity.md](type-identity.md); where content lives — the run-frame type
registry — is [type-registry.md](type-registry.md).

## Memoized subtype matching

Subtype outcomes — including signature subtyping, the most frequently checked relation this
design adds — are recorded as verdict edges on the run-frame type registry, keyed by
`(subject digest, candidate digest, relation)`, positive and negative outcomes alike. A module's structural satisfaction
check (`self_sig <: schema(sig)`) memoizes under the `SigSatisfies` relation with the module's
self-sig content digest and the signature's content digest as the key; because the subject
keys on interface content rather than the module mint, two modules with identical interfaces
share one cached verdict, and a repeat admissibility check is O(1). Dispatch
specificity between two distinct SIG slots reuses the same relation to order them (see
[modules.md § First-class modules](modules.md#first-class-modules)). Types are immutable, so
verdicts never invalidate; dropping an edge or a cold registry costs a re-walk, never a
wrong answer, and no verdict is observable to a koan program. Verdict scope is the run:
the registry drops with its run frame, so each run starts cold and warms itself. The
mechanism lives in
[type-identity.md § The memo registry](type-identity.md#the-memo-registry) and
[type-registry.md § Verdict edges memoize subtyping](type-registry.md#verdict-edges-memoize-subtyping).
