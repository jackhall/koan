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

use crate::machine::core::NodeId;
use crate::machine::core::{LexicalFrame, Scope, ScopeId};
use crate::machine::model::TypeIdentifier;
use crate::machine::model::{KType, TypeResolution};

impl<'step> Scope<'step> {
    /// Layer-2 scope-bound TypeIdentifier resolution memo. On miss, elaborates against
    /// `self` and writes the cache only when a [`FinalizeGate`] admits the result. The
    /// Park arm — elaborator-parked or gate-rejected — never writes the cache: caching
    /// mid-SCC would observe pre-close opaque identity.
    pub fn resolve_type_identifier(
        &self,
        te: &TypeIdentifier,
        chain: Option<std::rc::Rc<LexicalFrame>>,
    ) -> TypeResolution<&'step KType> {
        use crate::machine::model::{elaborate_type_identifier, Elaborator};
        // The cutoff this scope's bindings are gated against — also the memo key, so a
        // forward and a backward consumer never share a cached verdict.
        let cutoff = chain.as_ref().and_then(|c| c.index_for(self.id));
        if let Some(kt) = self.type_identifier_memo_get(te, cutoff) {
            return TypeResolution::Done(kt);
        }
        let mut elaborator = Elaborator::new(self).with_chain(chain);
        // A referenced type still in flight demotes this `Done` to a `Park`; `Park` /
        // `Unbound` forward unchanged.
        elaborate_type_identifier(&mut elaborator, te).and_then_done(|kt| {
            let pending = FinalizeGate { scope: self }.pending_producers(&kt);
            if pending.is_empty() {
                // The elaborated type is owned data; it stores into this scope's own region
                // through the single door and is cached there so a hit rebuilds the read carrier.
                let kt_ref: &'step KType = self.brand().alloc_ktype(kt);
                self.type_identifier_memo_insert(te.clone(), cutoff, kt_ref);
                TypeResolution::Done(kt_ref)
            } else {
                TypeResolution::Park(pending)
            }
        })
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
    fn pending_producers(&self, kt: &KType) -> Vec<NodeId> {
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
struct KTypeUserRefs<'b> {
    stack: Vec<&'b KType>,
}

impl<'b> KTypeUserRefs<'b> {
    fn of(kt: &'b KType) -> Self {
        Self { stack: vec![kt] }
    }
}

impl<'b> Iterator for KTypeUserRefs<'b> {
    type Item = (ScopeId, &'b str);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(kt) = self.stack.pop() {
            match kt {
                // A sealed nominal member yields its declaring `(scope_id, name)`.
                KType::SetRef { set, index } => {
                    let member = set.member(*index);
                    return Some((member.scope_id, member.name.as_str()));
                }
                // A SIG-declared or module self-sig yields its declaring `(scope_id, path)`; the
                // scopeless `:Module` mint (`SENTINEL`) borrows no user type, so it is a leaf and
                // yields nothing.
                KType::Signature { content, .. } if content.sig_id != ScopeId::SENTINEL => {
                    return Some((content.sig_id, content.path.as_str()));
                }
                KType::Signature { .. } => {}
                KType::AbstractType { source, name } => {
                    return Some((*source, name.as_str()));
                }
                KType::List { element, .. } => self.stack.push(element),
                KType::Dict { key, value, .. } => {
                    self.stack.push(value);
                    self.stack.push(key);
                }
                KType::Record { fields, .. } => {
                    for t in fields.values() {
                        self.stack.push(t);
                    }
                }
                KType::KFunction { params, ret, .. } => {
                    self.stack.push(ret);
                    for a in params.values() {
                        self.stack.push(a);
                    }
                }
                KType::ConstructorApply { ctor, args, .. } => {
                    for a in args.iter().rev() {
                        self.stack.push(a);
                    }
                    self.stack.push(ctor);
                }
                KType::Union { members, .. } => {
                    for m in members.iter().rev() {
                        self.stack.push(m);
                    }
                }
                // Leaves: no nested `KType`. `DeferredReturn` carries only a hashable
                // surface shadow, so it bottoms out here too.
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
