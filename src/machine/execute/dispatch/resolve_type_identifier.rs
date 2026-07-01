//! Scope-bound resolution of a surface [`TypeIdentifier`] into a region-allocated `&KType`.
//!
//! Read-only consumer of the bindings façade: never touches `data`, `functions`,
//! `placeholders`, `pending`, `out`, or `kind` — the read-only dependency is what
//! justifies the split from `scope.rs`.
//!
//! ## Invariant pinned here
//!
//! **The `type_identifier_memo` is monotonic and never caches a not-yet-sealed type.**
//! An entry is written only on the `Done` arm AND only when every user-type the
//! elaborated result references is finalized (absent from its owning scope's
//! `pending_types`). The `Park` arm — a referenced type still in flight — never writes the
//! cache, so a half-built identity cannot leak into a later memo hit.

use std::rc::Rc;

use crate::machine::core::kfunction::NodeId;
use crate::machine::core::{FrameSet, LexicalFrame, Scope, ScopeId};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::types::KType;

/// Outcome of [`Scope::resolve_type_identifier`]. Mirrors
/// [`crate::machine::model::types::ElabResult`] but `Done` carries an
/// region-allocated cache reference plus the resolved binding's home-omitted foreign reach (so a
/// read witnesses the `&KType` in place from it), and `Park` carries scheduler `NodeId`s.
pub enum TypeIdentifierResolution<'step> {
    Done {
        kt: &'step KType<'step>,
        reach: FrameSet,
    },
    Park(Vec<NodeId>),
    Unbound(String),
}

impl<'step> Scope<'step> {
    /// Layer-2 scope-bound TypeIdentifier resolution memo. On miss, runs
    /// [`crate::machine::model::types::elaborate_type_identifier`] against `self`, asks a
    /// [`FinalizeGate`] whether the result is safe to share, and writes the cache
    /// only when the gate admits. The Park arm — elaborator-parked or gate-rejected —
    /// never writes the cache: caching mid-SCC would observe pre-close opaque identity.
    pub fn resolve_type_identifier(
        &self,
        te: &TypeIdentifier,
        chain: Option<std::rc::Rc<LexicalFrame>>,
    ) -> TypeIdentifierResolution<'step> {
        use crate::machine::model::types::{elaborate_type_identifier, ElabResult, Elaborator};
        // The cutoff this scope's bindings are gated against — also the memo key, so a
        // forward and a backward consumer never share a cached verdict.
        let cutoff = chain.as_ref().and_then(|c| c.index_for(self.id));
        if let Some((kt, reach)) = self.type_identifier_memo_get(te, cutoff) {
            return TypeIdentifierResolution::Done { kt, reach };
        }
        let chain_for_reach = chain.clone();
        let mut elaborator = Elaborator::new(self).with_chain(chain);
        match elaborate_type_identifier(&mut elaborator, te) {
            ElabResult::Done(kt) => {
                let pending = FinalizeGate { scope: self }.pending_producers(&kt);
                if pending.is_empty() {
                    let kt_ref: &'step KType<'step> = self.brand().alloc_ktype(kt);
                    // A bare `TypeIdentifier` resolves to at most one `types` binding, so its reach is
                    // that binding's stored reach (empty for a builtin / owned type; the child-scope
                    // reach for a module). Cached alongside `kt` so a hit rebuilds the read carrier.
                    let reach = self.resolve_type_reach(te.as_str(), chain_for_reach.as_deref());
                    self.type_identifier_memo_insert(te.clone(), cutoff, kt_ref, reach.clone());
                    TypeIdentifierResolution::Done { kt: kt_ref, reach }
                } else {
                    TypeIdentifierResolution::Park(pending)
                }
            }
            ElabResult::Park(producers) => TypeIdentifierResolution::Park(producers),
            ElabResult::Unbound(msg) => TypeIdentifierResolution::Unbound(msg),
        }
    }
}

/// Outcome of [`resolve_type_leaf_carrier`] — the type-channel adaptation of
/// [`TypeIdentifierResolution`] for the three bare-leaf token call sites.
pub(crate) enum TypeLeafCarrier<'step> {
    /// The memoized `&KType` witnessed in place, ready to ride the type channel (`Carried::Type`) the
    /// same dispatch transport every other body consumes. `reach` is the resolved binding's
    /// home-omitted foreign reach, so the read site builds a self-contained carrier from it.
    Resolved {
        kt: &'step KType<'step>,
        reach: FrameSet,
    },
    /// The bare leaf names a still-finalizing type; the producer `NodeId`s the caller
    /// parks on (single-producer in practice, see the module-level invariant).
    Park(Vec<NodeId>),
    /// No binding for the leaf name.
    Unbound(String),
}

/// Resolve a bare leaf [`TypeIdentifier`] through the memoized, park-capable
/// [`Scope::resolve_type_identifier`] bridge and adapt the result into a type-channel carrier.
///
/// A resolved leaf yields the memoized `&KType` so a struct / union / module / Result /
/// signature type token reaches a constructor or ATTR call site through the same transport
/// every other body consumes. A leaf naming a not-yet-sealed type parks on the producers
/// the bridge surfaces, so a bare leaf never observes a half-sealed identity. A miss
/// surfaces `Unbound(name)`.
pub(crate) fn resolve_type_leaf_carrier<'step>(
    scope: &Scope<'step>,
    t: &TypeIdentifier,
    chain: Option<Rc<LexicalFrame>>,
) -> TypeLeafCarrier<'step> {
    match scope.resolve_type_identifier(t, chain) {
        // Witness the resolved `&KType` in place — no `alloc_ktype` re-home — carrying its stored
        // reach forward for the read site to seal from.
        TypeIdentifierResolution::Done { kt, reach } => TypeLeafCarrier::Resolved { kt, reach },
        TypeIdentifierResolution::Park(producers) => TypeLeafCarrier::Park(producers),
        TypeIdentifierResolution::Unbound(message) => TypeLeafCarrier::Unbound(message),
    }
}

/// Precondition value for the `type_identifier_memo` cache, naming the load-bearing
/// invariant *"no not-yet-sealed type may enter the memo"* as a type.
///
/// Admits a `KType` iff every top-level user-type it references is finalized in
/// its owning scope (absent from that scope's `pending_types`); otherwise returns
/// the producer `NodeId`s the caller parks on.
struct FinalizeGate<'view, 'step> {
    scope: &'view Scope<'step>,
}

impl<'view, 'step> FinalizeGate<'view, 'step> {
    /// Producer `NodeId`s the caller must park on; empty iff the gate admits.
    fn pending_producers(&self, kt: &KType<'_>) -> Vec<NodeId> {
        let mut pending: Vec<NodeId> = Vec::new();
        for (scope_id, name) in KTypeUserRefs::of(kt) {
            let Some(owner) = self.scope.ancestors().find(|s| s.id == scope_id) else {
                continue;
            };
            if !owner.bindings().pending_types().contains_key(name) {
                continue;
            }
            // Read the type placeholder straight from the kind-tagged map — not via
            // `lookup_type`, which would prefer the seal's pre-installed (still-unsealed)
            // `types` entry and miss the in-flight producer this gate must park on.
            if let Some(node_id) = owner.bindings().type_placeholder_producer(name) {
                if !pending.contains(&node_id) {
                    pending.push(node_id);
                }
            }
        }
        pending
    }
}

/// Iterator yielding every top-level user-type reference `(scope_id, name)` in a
/// `KType`.
///
/// **Set discipline** (load-bearing): a `SetRef` is a leaf — does NOT descend the
/// referenced member's schema, whose identity is `(set ptr, index)` and which may be
/// cyclic. The dependency a consumer parks on is the named binder itself; its schema's own
/// references are that binder's concern, resolved when it finalizes.
struct KTypeUserRefs<'b, 'step> {
    stack: Vec<&'b KType<'step>>,
}

impl<'b, 'step> KTypeUserRefs<'b, 'step> {
    fn of(kt: &'b KType<'step>) -> Self {
        Self { stack: vec![kt] }
    }
}

impl<'b, 'step> Iterator for KTypeUserRefs<'b, 'step> {
    type Item = (ScopeId, &'b str);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(kt) = self.stack.pop() {
            match kt {
                // A variant references its union member — same `(scope_id, name)` a bare
                // `SetRef` to that member yields; the `tag` selects within it and adds no
                // further user-type reference.
                KType::SetRef { set, index } | KType::Variant { set, index, .. } => {
                    let member = set.member(*index);
                    return Some((member.scope_id, member.name.as_str()));
                }
                KType::Signature { sig, .. } => {
                    return Some((sig.sig_id(), sig.path.as_str()));
                }
                KType::Module { module, .. } => {
                    return Some((module.scope_id(), module.path.as_str()));
                }
                KType::AbstractType { source, name } => {
                    return Some((source.scope_id(), name.as_str()));
                }
                KType::List(inner) => self.stack.push(inner),
                KType::Dict(k, v) => {
                    self.stack.push(v);
                    self.stack.push(k);
                }
                // Walk each field's type; `Record::values()` order is immaterial here.
                KType::Record(fields) => {
                    for t in fields.values() {
                        self.stack.push(t);
                    }
                }
                // Order is immaterial (the walker only collects the set of nested
                // user-type refs), and `Record::values()` is not double-ended, so no `.rev()`.
                KType::KFunction { params, ret } => {
                    self.stack.push(ret);
                    for a in params.values() {
                        self.stack.push(a);
                    }
                }
                KType::KFunctor { params, ret, .. } => {
                    self.stack.push(ret);
                    for p in params.values() {
                        self.stack.push(p);
                    }
                }
                KType::ConstructorApply { ctor, args } => {
                    for a in args.iter().rev() {
                        self.stack.push(a);
                    }
                    self.stack.push(ctor);
                }
                // Leaves / wildcards: no nested user‑type references at this level.
                // `DeferredReturn` carries only a hashable surface shadow (no nested
                // `KType`), so it bottoms out here.
                KType::Number
                | KType::Str
                | KType::Bool
                | KType::Null
                | KType::Identifier
                | KType::KExpression
                | KType::SigiledTypeExpr
                | KType::RecordType
                | KType::OfKind(_)
                | KType::Any
                | KType::DeferredReturn(_)
                | KType::SetLocal(_)
                | KType::RecursiveRef(_)
                | KType::Unresolved(_)
                | KType::RecursiveGroup(_) => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
