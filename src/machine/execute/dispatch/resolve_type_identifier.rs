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

use crate::machine::core::kfunction::NodeId;
use crate::machine::core::{LexicalFrame, Scope, ScopeId, TypeHit};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::types::{KType, SigSource, TypeResolution};

impl<'step> Scope<'step> {
    /// Layer-2 scope-bound TypeIdentifier resolution memo. On miss, elaborates against
    /// `self` and writes the cache only when a [`FinalizeGate`] admits the result. The
    /// Park arm — elaborator-parked or gate-rejected — never writes the cache: caching
    /// mid-SCC would observe pre-close opaque identity.
    pub fn resolve_type_identifier(
        &self,
        te: &TypeIdentifier,
        chain: Option<std::rc::Rc<LexicalFrame>>,
    ) -> TypeResolution<TypeHit<'step>> {
        use crate::machine::model::types::{elaborate_type_identifier, Elaborator};
        // The cutoff this scope's bindings are gated against — also the memo key, so a
        // forward and a backward consumer never share a cached verdict.
        let cutoff = chain.as_ref().and_then(|c| c.index_for(self.id));
        if let Some((kt, reach)) = self.type_identifier_memo_get(te, cutoff) {
            return TypeResolution::Done(TypeHit { kt, stored: reach });
        }
        let chain_for_reach = chain.clone();
        let mut elaborator = Elaborator::new(self).with_chain(chain);
        // A referenced type still in flight demotes this `Done` to a `Park`; `Park` /
        // `Unbound` forward unchanged.
        elaborate_type_identifier(&mut elaborator, te).and_then_done(|kt| {
            let pending = FinalizeGate { scope: self }.pending_producers(&kt);
            if pending.is_empty() {
                // A bare `TypeIdentifier` resolves to at most one binding, so its token is that
                // binding's stored token (empty for a builtin / owned type) — replayed whole with its
                // home-borrow bit, and minted *before* the alloc below so `kt`'s own residence audit
                // can see it. Cached alongside `kt` so a hit rebuilds the read carrier. A module head
                // lowers to `Signature { SelfOf }`, which borrows the module; the module is bound
                // value-side, so its child-scope reach comes off the `data` entry.
                let stored = self
                    .resolve_type_stored(te.as_str(), chain_for_reach.as_deref())
                    .or_else(|| match &kt {
                        KType::Signature {
                            sig: SigSource::SelfOf(_),
                            ..
                        } => self.resolve_value_stored(te.as_str(), chain_for_reach.as_deref()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let kt_ref: &'step KType<'step> = self
                    .alloc_ktype_reaching(kt, &stored)
                    .expect("resolve_type_identifier: kt must be covered by its own stored reach");
                self.type_identifier_memo_insert(te.clone(), cutoff, kt_ref, stored);
                TypeResolution::Done(TypeHit { kt: kt_ref, stored })
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
                // A sealed nominal member yields its declaring `(scope_id, name)`.
                KType::SetRef { set, index } => {
                    let member = set.member(*index);
                    return Some((member.scope_id, member.name.as_str()));
                }
                // A `Declared`/`SelfOf` signature yields its decl/module `(scope_id, path)`; the
                // empty signature borrows no user type, so it is a leaf and yields nothing.
                KType::Signature { sig, .. } => match sig {
                    SigSource::Declared(s) => return Some((s.sig_id(), s.path.as_str())),
                    SigSource::SelfOf(m) => return Some((m.scope_id(), m.path.as_str())),
                    SigSource::Empty => {}
                },
                KType::Module { module, .. } => {
                    return Some((module.scope_id(), module.path.as_str()));
                }
                KType::AbstractType { source, name } => {
                    return Some((source.scope_id(), name.as_str()));
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
                KType::KFunctor { params, ret, .. } => {
                    self.stack.push(ret);
                    for p in params.values() {
                        self.stack.push(p);
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
