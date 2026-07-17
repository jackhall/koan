# Type identity

The design of koan's type identity: identity is a wide content-hash
digest computed eagerly at construction; equality is one digest compare; a
thread-local registry memoizes subtype verdicts and is never load-bearing.
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
different corners of a program compare equal by digest. Signatures share the
rule: a `SIG` digests over its schema content and a module over its sealed
self-sig, so identical interfaces denote one signature type. Dispatch, matching,
and memo caches all inherit this unification.

## Opaque ascription is the generative exception

`:|` exists to hide representation, and hiding rides distinctness: each
application mints a fresh identity nonce and folds it into the digested
content, so two applications of the same signature member over the same
representation are distinct types. Generativity survives exactly where
abstraction demands it and nowhere else.

The sole remaining id-keyed leaf is `AbstractType`: its digest folds the
minting scope id, is stable within a run, and the order-independence property
above is scoped to types without such a leaf. A `Signature` is *not* id-keyed —
it digests by its source's schema content (member names, abstract members'
arity, and manifest-member / value-slot type digests), with references to the
schema's own abstract members canonicalized to a name leaf. Two textually
identical `SIG` declarations, and two modules with identical interfaces, are
therefore one type. Opacity still rides `AbstractType`: the abstract-type slots
a `:|` view mints stay id-keyed, so they flow into a self-sig's content digest
unchanged and two opaque views stay distinct.

## Content lives on the type value

A type's structure is owned by its `KType`: record fields in `Record`, union
members in `Union`, function shapes in `KFunction`, and nominal-set schemas
behind the `RecursiveSet` its `SetRef`s share. Content access — `kind_of`,
`name`, schema projection, residence audits — reads through the value in hand,
and the registry below holds no content, so nothing about reading a type
depends on any table being reachable. A type riding a lifted value travels by
ownership, and its digest travels with it.

## The memo registry

A subtype verdict is a pure function of a `(subject digest, candidate digest,
relation)` key: a digest is content identity, so once a verdict is computed it
never changes for the life of the process, and any caching granularity —
per-frame, per-thread, whole-process — is observationally identical. koan caches
verdicts in a **thread-local flat LRU** — one `LruCache` per OS thread
([`type_memos.rs`](../../src/machine/model/types/type_memos.rs)), consulted
before a structural walk and filled after one. An explicit `Relation` tag keeps
two questions in one cache: `MoreSpecific` (the strict specificity walk) and
`SigSatisfies` (module/signature structural satisfaction — see
[module-values-and-type-identity.md § Memoized subtype matching](module-values-and-type-identity.md#memoized-subtype-matching)).
`KType` itself carries no registry reference; the predicate call sites reach the
thread-local directly.

The registry is a cache, never a soundness mechanism:

- Every verdict is re-derivable by the structural walk it memoizes, so eviction
  (the LRU bound) or a cold thread costs a re-walk, never a wrong answer — the
  walk stays the source of truth. No verdict is observable to a koan program.
- The one purity hazard is a pre-seal `RecursiveSet`, whose digest is a
  pointer-derived transient rather than a content digest. An insert guard
  (`memo_safe`) keeps any type embedding such a set out of the cache, so a stale
  hit is impossible; lookups need no guard, since nothing unsafe was inserted.
- Thread-local is lock-free under ready-node parallelism and every sketched
  future concurrency primitive — a cold worker thread simply re-walks and warms
  its own cache.
