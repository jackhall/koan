//! A thread-local flat LRU cache of subtype verdicts, keyed by `(subject digest, candidate
//! digest, relation)`. `TypeDigest` is content identity (`type_digest.rs`), so a subtype
//! verdict over a digest pair is a pure function for the life of the process: once computed
//! it never changes, and any granularity — per-frame, per-thread, whole-process — is
//! observationally identical. This registry picks thread-local: one `LruCache` per OS thread,
//! consulted before a structural walk and filled after one.
//!
//! The cache is never load-bearing. Eviction (the LRU bound) or a fresh thread (a cold
//! registry) costs a re-walk of the structural predicate, never a wrong answer — the walk
//! itself remains the source of truth. The one soundness hazard is a pre-seal `RecursiveSet`,
//! whose digest is a pointer-derived transient rather than a content digest (`type_digest.rs`,
//! `feed_set_identity`'s `None` arm); [`memo_safe`] keeps such digests out of the cache on
//! insert, so a lookup never needs its own guard — no insert, no hit, no staleness.

use std::cell::RefCell;
use std::num::NonZeroUsize;

use lru::LruCache;

use super::ktype::KType;
use super::type_digest::TypeDigest;

/// Which subtype question a cached verdict answers. `MoreSpecific` is
/// `KType::is_more_specific_than`'s strict specificity walk; `SigSatisfies` is
/// `sig_subtype(schema-of-subject, schema-of-candidate).is_ok()`, where "schema-of" a
/// module-identity digest is the module's self-sig and "schema-of" a signature-identity
/// digest is `SigSchema::of_sig`. The two relations never alias — each digest domain
/// (`TAG_MODULE` / `TAG_SIGNATURE` / the composite tags) is disjoint by construction — but the
/// enum still keys the cache explicitly so the two questions never share an entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Relation {
    MoreSpecific,
    // wired in Phase 3 (`Module::structurally_satisfies`, `SigSource::satisfied_by_module`)
    #[allow(dead_code)]
    SigSatisfies,
}

/// Verdict capacity: entries evicted beyond this bound simply cost a re-walk on next ask.
/// 65,536 verdicts is worst case ~6 MB per thread (`(u128, u128, Relation) -> bool` plus the
/// `lru` crate's intrusive list/map overhead).
const MEMO_CAPACITY: usize = 65_536;

/// A flat LRU map from a digest-pair-and-relation key to its cached verdict. A plain struct —
/// not itself thread-local — so unit tests can build one with a tiny capacity and exercise
/// eviction/recency directly; the thread-local singleton below is the only production use.
pub(crate) struct TypeMemoCache {
    cache: LruCache<(TypeDigest, TypeDigest, Relation), bool>,
}

impl TypeMemoCache {
    pub(crate) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            cache: LruCache::new(capacity),
        }
    }

    /// Consult the cache, bumping the entry's recency on a hit.
    pub(crate) fn lookup(
        &mut self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
    ) -> Option<bool> {
        let hit = self.cache.get(&(subject, candidate, relation)).copied();
        #[cfg(test)]
        {
            if hit.is_some() {
                MEMO_HITS.with(|c| c.set(c.get() + 1));
            } else {
                MEMO_MISSES.with(|c| c.set(c.get() + 1));
            }
        }
        hit
    }

    /// Record `verdict` for the key, possibly evicting the least-recently-used entry.
    pub(crate) fn insert(
        &mut self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
        verdict: bool,
    ) {
        self.cache.put((subject, candidate, relation), verdict);
    }
}

thread_local! {
    static TYPE_MEMOS: RefCell<TypeMemoCache> = RefCell::new(TypeMemoCache::new(
        NonZeroUsize::new(MEMO_CAPACITY).expect("MEMO_CAPACITY is nonzero"),
    ));
}

/// Thread-local lookup. The `RefCell` borrow is confined to this one call — never held across
/// the structural walk a miss falls back to.
pub(crate) fn lookup(
    subject: TypeDigest,
    candidate: TypeDigest,
    relation: Relation,
) -> Option<bool> {
    TYPE_MEMOS.with(|m| m.borrow_mut().lookup(subject, candidate, relation))
}

/// Thread-local insert. The `RefCell` borrow is confined to this one call.
pub(crate) fn insert(
    subject: TypeDigest,
    candidate: TypeDigest,
    relation: Relation,
    verdict: bool,
) {
    TYPE_MEMOS.with(|m| m.borrow_mut().insert(subject, candidate, relation, verdict));
}

/// The insert guard: `false` if `kt` contains any `SetRef` / `RecursiveGroup` over an
/// unsealed set (a pointer-transient digest, unsafe to memoize — see the module doc), else
/// recurses into every composite child position. All other variants — every leaf, every
/// id-keyed variant (`Module`, `AbstractType`), and a `SetRef`/`RecursiveGroup` over a
/// *sealed* set — are safe. The guard runs only on insert; a lookup needs no guard, since an
/// unsafe verdict is never inserted in the first place.
pub(crate) fn memo_safe(kt: &KType<'_>) -> bool {
    match kt {
        // The one hazard: a pre-seal set digests by `Rc` pointer address, which can be reused
        // once freed. A sealed set's digest is content-derived and safe.
        KType::SetRef { set, .. } => set.digest().is_some(),
        KType::RecursiveGroup(set) => set.digest().is_some(),

        // Composite variants: safe only if every child position is safe.
        KType::List { element, .. } => memo_safe(element),
        KType::Dict { key, value, .. } => memo_safe(key) && memo_safe(value),
        KType::Record { fields, .. } => fields.iter().all(|(_, field)| memo_safe(field)),
        KType::KFunction { params, ret, .. } | KType::KFunctor { params, ret, .. } => {
            params.iter().all(|(_, p)| memo_safe(p)) && memo_safe(ret)
        }
        KType::Union { members, .. } => members.iter().all(memo_safe),
        KType::ConstructorApply { ctor, args, .. } => memo_safe(ctor) && args.iter().all(memo_safe),
        // `sig` itself is an id-leaf (`Declared`/`SelfOf`/`Empty` carry no nested `KType`);
        // only the `WITH`-pinned slot types need recursing.
        KType::Signature { pinned_slots, .. } => pinned_slots.iter().all(|(_, kt)| memo_safe(kt)),

        // Leaves and id-keyed variants: no nested `KType`, no unsealed set.
        KType::Number
        | KType::Str
        | KType::Bool
        | KType::Null
        | KType::Identifier
        | KType::KExpression
        | KType::SigiledTypeExpr
        | KType::RecordType
        | KType::Any
        | KType::OfKind(_)
        | KType::DeferredReturn(_)
        | KType::SetLocal(_)
        | KType::RecursiveRef(_)
        | KType::Unresolved(_)
        | KType::Module { .. }
        | KType::AbstractType { .. } => true,
    }
}

#[cfg(test)]
thread_local! {
    static MEMO_HITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MEMO_MISSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counter-verified hit assertions in the predicate tests.
#[cfg(test)]
pub(crate) fn hit_count() -> usize {
    MEMO_HITS.with(|c| c.get())
}

/// Counter-verified miss assertions in the predicate tests.
#[cfg(test)]
pub(crate) fn miss_count() -> usize {
    MEMO_MISSES.with(|c| c.get())
}

/// Clear the thread-local cache and zero both counters. Tests reset at start; the test
/// harness gives each test its own thread, but callers should not rely on that alone.
#[cfg(test)]
pub(crate) fn reset() {
    TYPE_MEMOS.with(|m| {
        *m.borrow_mut() =
            TypeMemoCache::new(NonZeroUsize::new(MEMO_CAPACITY).expect("MEMO_CAPACITY is nonzero"));
    });
    MEMO_HITS.with(|c| c.set(0));
    MEMO_MISSES.with(|c| c.set(0));
}

#[cfg(test)]
mod tests;
