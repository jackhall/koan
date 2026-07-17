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

Ascription operators construct views; they do not grant admission: `:!` asserts satisfaction and yields a
transparent view; `:|` is generative, minting fresh abstract-type identities per
application. `AbstractType` identity is id-keyed — `KType` holds no `&Module`.

## The empty signature

The empty signature is the top of the module lattice: every module's self-sig is a subtype
of it. The `Module` surface keyword lowers to it, so "any module" slots (`USING`'s receiver)
are signature-typed like every other module slot — the module/signature story has no
kind-wildcard exception.

## Content-addressed type identity

Type identity is a wide content-hash digest, computed eagerly bottom-up when the type is
created, from the type's content alone — no raw-pointer identity in `KType`, no dependence
on interning order, no shared interner. The digest is wide enough that equality is one
digest compare with no repair path; opaque ascription stays generative by minting a
per-application nonce into the digested content. The full design — including where type
content lives and the thread-local memo registry — is [type-identity.md](type-identity.md).

## Memoized subtype matching

Subtype outcomes — including signature subtyping, the most frequently checked relation this
design adds — are cached in a thread-local flat LRU keyed by `(subject digest, candidate
digest, relation)`, positive and negative outcomes alike. A module's structural satisfaction
check (`self_sig <: schema(sig)`) memoizes under the `SigSatisfies` relation with the module's
and signature's digests as the key; a repeat admissibility check is then O(1). Dispatch
specificity between two distinct SIG slots reuses the same relation to order them (see
[modules.md § First-class modules](modules.md#first-class-modules)). Types are immutable, so
verdicts never invalidate; LRU eviction or a cold thread costs a re-walk, never a wrong
answer, and no verdict is observable to a koan program. The mechanism, capacity, and the
insert guard for pre-seal pointer transients live in
[type-identity.md § The memo registry](type-identity.md#the-memo-registry).
