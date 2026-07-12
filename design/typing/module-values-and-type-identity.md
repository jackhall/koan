# Module values and type identity

The end-state design of koan's module representation and type identity, in five facets that
form one picture: modules are values typed by signatures; satisfaction is structural
subtyping against a creation-time principal signature; `:Module` is the empty signature;
type identity is a content-hash digest; subtype outcomes are memoized on that digest.
[Open work](#open-work) lists the implementing roadmap items; until they ship,
[modules.md](modules.md) and [ktype/README.md](ktype/README.md) describe current behavior.

## Modules are values

A module is a runtime value — `KObject::Module` — never a type. Nothing is typed *by* a
module, so `KType` carries no module variant: the type channel contains only things that can
type a field. Module names are snake_case value identifiers (`int_ord`); `MODULE` binds
value-side. Signature names use the Type-token spelling with no suffix (`Ordered`), making
the Type-token namespace exactly the set of things that type fields. Member access is ATTR
over the value. A type expression whose head names a module — e.g. a return type `er.Type`,
where `er` is a module-valued parameter — resolves by reading the named type member off the
module value. A concrete module's identity is never a
slot or return type — slots and returns name signatures.

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

Ascription operators construct views; they do not grant admission: `:!` asserts satisfaction and yields a
transparent view; `:|` is generative, minting fresh abstract-type identities per
application. `AbstractType` identity is id-keyed — `KType` holds no `&Module`.

## The empty signature

The empty signature is the top of the module lattice: every module's self-sig is a subtype
of it. The `Module` surface keyword lowers to it, so "any module" slots (`USING`'s receiver)
are signature-typed like every other module slot — the module/signature story has no
kind-wildcard exception.

## Content-addressed type identity

Type identity is a wide content-hash digest (`Unique(u128) | Collided(u128)`), computed
bottom-up when the type is created, from the type's content alone — no raw-pointer identity
in `KType`, no dependence on interning order, thread-local tables that merge without a lock.
A per-frame `digest → type` table detects collisions (two distinct types with one digest)
and repairs by tagging the newcomer `Collided`, never by changing a hash. Equality is a
`u128` compare when both sides are `Unique`, and a structural walk otherwise.

## Memoized subtype matching

Subtype outcomes — including signature subtyping, the most frequently checked relation this
design adds — are cached per
type in its registry entry, keyed by the candidate supertype's digest, positive and negative
outcomes alike. Types are immutable, so entries never invalidate. A repeat admissibility
check is O(1).

## Open work

- [Structural satisfaction](../../roadmap/type_memos/structural-satisfaction.md)
- [KObject module carrier](../../roadmap/type_memos/kobject-module-carrier.md)
- [Value-head type paths](../../roadmap/type_memos/value-head-type-paths.md)
- [Module naming flip](../../roadmap/type_memos/module-naming-flip.md)
- [Content-addressed type identity](../../roadmap/type_memos/type-identity-registry.md)
- [Memoized subtype matching](../../roadmap/type_memos/memoized-subtype-matching.md)
