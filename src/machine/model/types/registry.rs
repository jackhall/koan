//! The run's type registry: a flat map of subtype verdicts keyed by `(subject digest, candidate
//! digest, relation)`. `TypeDigest` is content identity (`type_digest.rs`), so a subtype verdict
//! over a digest pair is a pure function — once computed it never changes — and any granularity
//! is observationally identical. The registry picks the run: one [`TypeRegistry`] hangs off the
//! scheduler-owned run frame (`CallFrame::type_registry`), reached by reference through the
//! execution context, so there is no process-global state and the map drops with the run.
//!
//! Verdicts are never load-bearing. A cold registry costs a re-walk of the structural predicate,
//! never a wrong answer — the walk itself remains the source of truth. The one soundness hazard is
//! a pre-seal `RecursiveSet`, whose digest is a pointer-derived transient rather than a content
//! digest (`type_digest.rs`, `feed_set_identity`'s `None` arm); [`digest_is_content`] keeps such
//! digests out of the map on record, so a lookup needs no guard of its own — nothing recorded,
//! nothing to hit.
//!
//! See [design/typing/type-registry.md](../../../../design/typing/type-registry.md).

use std::cell::RefCell;
use std::collections::HashMap;

use super::ktype::KType;
use super::type_digest::TypeDigest;

/// Which subtype question a recorded verdict answers. `MoreSpecific` is
/// `KType::is_more_specific_than`'s strict specificity walk; `SigSatisfies` is
/// `sig_subtype(schema-of-subject, schema-of-candidate).is_ok()`, where "schema-of" a
/// module-identity digest is the module's self-sig and "schema-of" a signature-identity
/// digest is `SigSchema::of_sig`. The two relations never alias — each digest domain
/// (`TAG_MODULE` / `TAG_SIGNATURE` / the composite tags) is disjoint by construction — but the
/// enum still keys the map explicitly so the two questions never share an entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Relation {
    MoreSpecific,
    SigSatisfies,
}

/// The run-scoped store of subtype verdicts. Interior mutability via `RefCell`; every borrow is
/// confined to a single method call, so no borrow spans the structural walk a miss falls back to.
/// The map is uncapped: it drops with the run frame that owns it, so growth is bounded by the run.
pub struct TypeRegistry {
    verdicts: RefCell<HashMap<(TypeDigest, TypeDigest, Relation), bool>>,
    #[cfg(test)]
    hits: std::cell::Cell<usize>,
    #[cfg(test)]
    misses: std::cell::Cell<usize>,
}

impl TypeRegistry {
    pub(crate) fn new() -> Self {
        Self {
            verdicts: RefCell::new(HashMap::new()),
            #[cfg(test)]
            hits: std::cell::Cell::new(0),
            #[cfg(test)]
            misses: std::cell::Cell::new(0),
        }
    }

    /// Consult the registry for a recorded verdict.
    pub(crate) fn verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
    ) -> Option<bool> {
        let hit = self
            .verdicts
            .borrow()
            .get(&(subject, candidate, relation))
            .copied();
        #[cfg(test)]
        {
            if hit.is_some() {
                self.hits.set(self.hits.get() + 1);
            } else {
                self.misses.set(self.misses.get() + 1);
            }
        }
        hit
    }

    /// Record `verdict` for the key. Negative verdicts are recorded exactly as positive ones.
    pub(crate) fn record_verdict(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
        verdict: bool,
    ) {
        self.verdicts
            .borrow_mut()
            .insert((subject, candidate, relation), verdict);
    }

    /// Counter-verified hit assertions in the predicate tests. A fresh registry per run replaces
    /// resetting, so there is no counter reset.
    #[cfg(test)]
    pub(crate) fn hit_count(&self) -> usize {
        self.hits.get()
    }

    /// Counter-verified miss assertions in the predicate tests.
    #[cfg(test)]
    pub(crate) fn miss_count(&self) -> usize {
        self.misses.get()
    }
}

/// The record guard: `false` if `kt` contains any `SetRef` / `RecursiveGroup` over an unsealed set
/// (a pointer-transient digest, unsafe to record — see the module doc), else recurses into every
/// composite child position. All other variants — every leaf, every id-keyed variant (`Module`,
/// `AbstractType`), and a `SetRef`/`RecursiveGroup` over a *sealed* set — carry content digests.
/// The guard runs only on record; a lookup needs no guard, since an unsafe verdict is never
/// recorded in the first place.
pub(crate) fn digest_is_content(kt: &KType) -> bool {
    match kt {
        // The one hazard: a pre-seal set digests by `Rc` pointer address, which can be reused
        // once freed. A sealed set's digest is content-derived and safe.
        KType::SetRef { set, .. } => set.digest().is_some(),
        KType::RecursiveGroup(set) => set.digest().is_some(),

        // Composite variants: safe only if every child position is safe.
        KType::List { element, .. } => digest_is_content(element),
        KType::Dict { key, value, .. } => digest_is_content(key) && digest_is_content(value),
        KType::Record { fields, .. } => fields.iter().all(|(_, field)| digest_is_content(field)),
        KType::KFunction { params, ret, .. } => {
            params.iter().all(|(_, p)| digest_is_content(p)) && digest_is_content(ret)
        }
        KType::Union { members, .. } => members.iter().all(digest_is_content),
        KType::ConstructorApply { ctor, args, .. } => {
            digest_is_content(ctor) && args.iter().all(digest_is_content)
        }
        // `content` is owned schema data with no nested `KType` of its own; only the
        // `WITH`-pinned slot types need recursing.
        KType::Signature { pinned_slots, .. } => {
            pinned_slots.iter().all(|(_, kt)| digest_is_content(kt))
        }

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
        | KType::AbstractType { .. } => true,
    }
}

#[cfg(test)]
mod tests;
