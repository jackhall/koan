//! Scope-bound resolution of a surface [`TypeName`] into an arena-allocated `&KType`.
//!
//! Read-only consumer of the bindings façade: never touches `data`, `functions`,
//! `placeholders`, `pending`, `out`, or `kind` — the read-only dependency is what
//! justifies the split from `scope.rs`.
//!
//! ## Invariant pinned here
//!
//! **The `type_expr_memo` is monotonic and never caches a not-yet-sealed type.**
//! An entry is written only on the `Done` arm AND only when every user-type the
//! elaborated result references is finalized (absent from its owning scope's
//! `pending_types`). The `Park` arm — a referenced type still in flight — never writes the
//! cache, so a half-built identity cannot leak into a later memo hit.

use std::rc::Rc;

use crate::machine::core::kfunction::NodeId;
use crate::machine::core::{LexicalFrame, Scope, ScopeId};
use crate::machine::model::ast::TypeName;
use crate::machine::model::types::KType;

/// Outcome of [`Scope::resolve_type_expr`]. Mirrors
/// [`crate::machine::model::types::ElabResult`] but `Done` carries an
/// arena-allocated cache reference and `Park` carries scheduler `NodeId`s.
pub enum ResolveTypeExprOutcome<'step> {
    Done(&'step KType<'step>),
    Park(Vec<NodeId>),
    Unbound(String),
}

impl<'step> Scope<'step> {
    /// Layer-2 scope-bound TypeName resolution memo. On miss, runs
    /// [`crate::machine::model::types::elaborate_type_expr`] against `self`, asks a
    /// [`FinalizeGate`] whether the result is safe to share, and writes the cache
    /// only when the gate admits. The Park arm — elaborator-parked or gate-rejected —
    /// never writes the cache: caching mid-SCC would observe pre-close opaque identity.
    pub fn resolve_type_expr(
        &self,
        te: &TypeName,
        chain: Option<std::rc::Rc<LexicalFrame>>,
    ) -> ResolveTypeExprOutcome<'step> {
        use crate::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator};
        // The cutoff this scope's bindings are gated against — also the memo key, so a
        // forward and a backward consumer never share a cached verdict.
        let cutoff = chain.as_ref().and_then(|c| c.index_for(self.id));
        if let Some(kt) = self.type_expr_memo_get(te, cutoff) {
            return ResolveTypeExprOutcome::Done(kt);
        }
        let mut elaborator = Elaborator::new(self).with_chain(chain);
        match elaborate_type_expr(&mut elaborator, te) {
            ElabResult::Done(kt) => {
                let pending = FinalizeGate { scope: self }.pending_producers(&kt);
                if pending.is_empty() {
                    let kt_ref: &'step KType<'step> = self.arena.alloc_ktype(kt);
                    self.type_expr_memo_insert(te.clone(), cutoff, kt_ref);
                    ResolveTypeExprOutcome::Done(kt_ref)
                } else {
                    ResolveTypeExprOutcome::Park(pending)
                }
            }
            ElabResult::Park(producers) => ResolveTypeExprOutcome::Park(producers),
            ElabResult::Unbound(msg) => ResolveTypeExprOutcome::Unbound(msg),
        }
    }
}

/// Outcome of [`resolve_type_leaf_carrier`] — the type-channel adaptation of
/// [`ResolveTypeExprOutcome`] for the three bare-leaf token call sites.
pub(crate) enum TypeLeafCarrier<'step> {
    /// The memoized `&KType`, ready to ride the type channel (`Carried::Type`) the same
    /// dispatch transport every other body consumes.
    Resolved(&'step KType<'step>),
    /// The bare leaf names a still-finalizing type; the producer `NodeId`s the caller
    /// parks on (single-producer in practice, see the module-level invariant).
    Park(Vec<NodeId>),
    /// No binding for the leaf name.
    Unbound(String),
}

/// Resolve a bare leaf [`TypeName`] through the memoized, park-capable
/// [`Scope::resolve_type_expr`] bridge and adapt the result into a type-channel carrier.
///
/// A resolved leaf yields the memoized `&KType` so a struct / union / module / Result /
/// signature type token reaches a constructor or ATTR call site through the same transport
/// every other body consumes. A leaf naming a not-yet-sealed type parks on the producers
/// the bridge surfaces, so a bare leaf never observes a half-sealed identity. A miss
/// surfaces `Unbound(name)`.
pub(crate) fn resolve_type_leaf_carrier<'step>(
    scope: &Scope<'step>,
    t: &TypeName,
    chain: Option<Rc<LexicalFrame>>,
) -> TypeLeafCarrier<'step> {
    match scope.resolve_type_expr(t, chain) {
        ResolveTypeExprOutcome::Done(kt) => {
            TypeLeafCarrier::Resolved(scope.arena.alloc_ktype(kt.clone()))
        }
        ResolveTypeExprOutcome::Park(producers) => TypeLeafCarrier::Park(producers),
        ResolveTypeExprOutcome::Unbound(message) => TypeLeafCarrier::Unbound(message),
    }
}

/// Precondition value for the `type_expr_memo` cache, naming the load-bearing
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
            // `chain_cutoff = None` because this is producer-dependency tracking, not
            // consumer-visibility enforcement. A `Value`-arm hit would mean the
            // named type already finalized, which the `pending_types` check above
            // rules out for any name reaching this branch.
            if let Some(crate::machine::core::Resolution::Placeholder(node_id)) =
                owner.bindings().lookup_value(name, None)
            {
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
