# Type identity

The design of koan's type identity: identity is a wide content-hash
digest computed eagerly at construction; equality is one digest compare; the
type registry memoizes subtype verdicts as graph edges and is never
load-bearing. [ktype/README.md](ktype/README.md) is the variant-level
companion; [type-registry.md](type-registry.md) is the storage story.

## Identity is an eager content digest

Every `KType` carries a wide digest, computed bottom-up when the type is
constructed — children carry theirs, so each node's digest is shallow work at
build time. A recursive set digests at seal, over its finite SCC presentation
(sibling references canonicalized to their index literal); a member's identity
is `(set digest, index)`.

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

## Content lives in the registry

A type's structure — record fields, union members, function shapes, nominal
schemas — is owned by the run-frame type registry, and a `KType` is the type's
content digest — a `Copy` handle into the registry's digest-keyed node store.
Content access — `kind_of`, `name`, schema projection — reads through the
registry reference the execution context carries; the handle *is* the digest,
so identity checks never touch content at all. [type-registry.md](type-registry.md) carries the full storage
model: nodes, labeled composition edges, and registry ownership.

## The memo registry

A subtype verdict is a pure function of a `(subject digest, candidate digest,
relation)` key: a digest is content identity, so once a verdict is computed it
never changes for the life of the registry, and any caching granularity is
observationally identical. The type registry records verdicts as **edges**
between the nodes they relate — the same graph that owns the content — labeled
by an explicit `Relation` tag that keeps two questions apart: `MoreSpecific`
(the strict specificity walk) and `SigSatisfies` (module/signature structural
satisfaction — see
[module-values-and-type-identity.md § Memoized subtype matching](module-values-and-type-identity.md#memoized-subtype-matching)).
A predicate consults the graph before a structural walk and records the
outcome after one, positive and negative alike.

The verdict edges are a cache, never a soundness mechanism:

- Every verdict is re-derivable by the structural walk it memoizes, so
  dropping an edge — or asking in a fresh registry — costs a re-walk, never a
  wrong answer; the walk stays the source of truth. No verdict is observable
  to a koan program.
- Pre-seal recursive-set state never reaches the registry (the builder interns
  only sealed content — [type-registry.md § Recursive sets are cyclic
  subgraphs](type-registry.md#recursive-sets-are-cyclic-subgraphs)), so every
  recorded verdict is keyed by true content digests and no insert guard is
  needed.
- Each future worker thread's run frame owns its own registry, so verdict
  memoization is lock-free under every sketched concurrency primitive — a cold
  registry simply re-walks and warms itself.

## Open work

- [Interned type content behind Copy handles](../../roadmap/type_memos/interned-type-content.md)
  — ships the rest of the registry storage model
  ([type-registry.md](type-registry.md)): "Content lives in the registry"
  (today content is owned by each `KType` value, and the recording guard
  `digest_is_content` stands in for the builder's pre-seal exclusion). The
  identity sections — eager digests, one-compare equality, generative opaque
  ascription — and "The memo registry" above are shipped and do not depend on
  this item.
