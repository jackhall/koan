# Type identity

The end-state design of koan's type identity: identity is a wide content-hash
digest computed eagerly at construction; equality is one digest compare; a
per-call-frame registry memoizes subtype verdicts and is never load-bearing.
The identity and generativity model below has shipped; the memo registry is the
one still-future half, tracked under [Open work](#open-work).
[ktype/README.md](ktype/README.md) is the variant-level companion.

## Identity is an eager content digest

Every `KType` carries a wide digest, computed bottom-up when the type is
constructed — children carry theirs, so each node's digest is shallow work at
build time. A recursive set digests at seal, over its finite SCC presentation
(`SetLocal(i)` as the index literal); a member's identity is
`(set digest, index)`.

The digest is a pure function of type content, so it is order-independent: two
independently built types with the same content have the same digest, with no
shared interner, no global table, and no reconciliation step. This is what
keeps identity local — a future parallel runtime mints digests thread-locally
and joins registries without a lock.

The digest *is* the truth: equality is an unconditional digest compare, hashing
keys on the digest, and no repair path exists. The width is chosen so that an
accidental collision is less likely than a hardware fault — the same footing on
which content-addressed systems like git treat hash equality as identity.

## Structurally identical declarations are one type

Because identity is content, two declarations with the same structure denote
the same type: a `NEWTYPE` in an FN body elaborated on every call yields the
*same* type each time, and equal record, union, or function types built in
different corners of a program compare equal by digest. Dispatch, matching, and
memo caches all inherit this unification.

## Opaque ascription is the generative exception

`:|` exists to hide representation, and hiding rides distinctness: each
application mints a fresh identity nonce and folds it into the digested
content, so two applications of the same signature member over the same
representation are distinct types. Generativity survives exactly where
abstraction demands it and nowhere else.

Minted leaves also appear where a schema embeds an id-keyed type (`Signature`,
`AbstractType`): those digests are stable within a run, and the
order-independence property above is scoped to types without minted leaves.

## Content lives on the type value

A type's structure is owned by its `KType`: record fields in `Record`, union
members in `Union`, function shapes in `KFunction`, and nominal-set schemas
behind the `RecursiveSet` its `SetRef`s share. Content access — `kind_of`,
`name`, schema projection, residence audits — reads through the value in hand,
and the registry below holds no content, so nothing about reading a type
depends on any table being reachable. A type riding a lifted value travels by
ownership, and its digest travels with it.

## The memo registry

Each call frame owns a registry: a map from digest to a memo entry holding
subtype and satisfaction verdicts (see
[module-values-and-type-identity.md § Memoized subtype matching](module-values-and-type-identity.md#memoized-subtype-matching)).
The registry is reached through the dispatch/step context at the predicate
call sites; `KType` itself carries no registry reference.

The registry is a cache, never a soundness mechanism:

- Every verdict is re-derivable by the structural walk it memoizes, so dropping
  any entry — or the whole registry — at any moment is semantically invisible.
- Nothing flows caller-ward or callee-ward for correctness. Arguments and
  lexical reads need no registry hand-off; a callee that misses re-derives and
  warms its own frame's registry.
- At frame exit the registry dies with the frame. Merging still-warm entries
  into the caller's registry on lift is an optimization the design permits, not
  an obligation.

## Open work

- [Memoized subtype matching](../../roadmap/type_memos/memoized-subtype-matching.md)
