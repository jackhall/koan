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
//!
//! In-flight-ness is decided per reference kind. A nominal member in flight is named by a relative
//! `Sibling` handle, which is meaningful only against the **declaration window** that minted it —
//! the nearest one on this scope's chain. The gate resolves the index to a member name there, then
//! walks for the scope that both carries that same window and holds the name in `pending_types`.
//! Window identity is what the ptr-equality does here: it stops an unrelated same-named
//! declaration, which opens its own window, from capturing the reference. A sealed member carries
//! an absolute handle and no window, so it is never in flight. A SIG-declared or abstract slot is
//! identified by the declaring scope id its node records.

use crate::machine::core::NodeId;
use crate::machine::core::{LexicalFrame, Scope, ScopeId};
use crate::machine::model::TypeIdentifier;
use crate::machine::model::{KType, TypeNode, TypeRegistry, TypeResolution};

impl<'step> Scope<'step> {
    /// Layer-2 scope-bound TypeIdentifier resolution memo. On miss, elaborates against
    /// `self` and writes the cache only when a [`FinalizeGate`] admits the result. The
    /// Park arm — elaborator-parked or gate-rejected — never writes the cache: caching
    /// mid-window would observe pre-seal opaque identity.
    pub fn resolve_type_identifier(
        &self,
        te: &TypeIdentifier,
        chain: Option<std::rc::Rc<LexicalFrame>>,
        types: &TypeRegistry,
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
        elaborate_type_identifier(&mut elaborator, te, types).and_then_done(|kt| {
            let pending = FinalizeGate { scope: self, types }.pending_producers(kt);
            if pending.is_empty() {
                // The handle is `Copy`, but the memo hands out `&'step KType`, so it stores into
                // this scope's own region through the single door and is cached there.
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
///
/// Both probes read the type placeholder straight from the kind-tagged map — not via
/// `lookup_type`, which would prefer a binding this gate must look past to find the in-flight
/// producer.
struct FinalizeGate<'view, 'step> {
    scope: &'view Scope<'step>,
    types: &'view TypeRegistry,
}

impl FinalizeGate<'_, '_> {
    /// Producer `NodeId`s the caller must park on; empty iff the gate admits.
    fn pending_producers(&self, kt: KType) -> Vec<NodeId> {
        let mut pending: Vec<NodeId> = Vec::new();
        for user_ref in user_type_refs(kt, self.types) {
            let producer = match user_ref {
                UserTypeRef::Sibling { index } => self.member_producer(index),
                UserTypeRef::Declared { scope_id, name } => self.declared_producer(scope_id, &name),
            };
            if let Some(node_id) = producer {
                if !pending.contains(&node_id) {
                    pending.push(node_id);
                }
            }
        }
        pending
    }

    /// The in-flight producer of the member a relative sibling reference names, or `None`.
    ///
    /// The index means whatever the **nearest** open window says it means, because that is the
    /// window the elaborator minted it against. Resolving it there and then requiring the pending
    /// scope to carry that *same* window is what keeps a same-named in-flight declaration of a
    /// different type from capturing this reference — an unrelated declaration of the name opens a
    /// window of its own, which is not this one.
    fn member_producer(&self, index: usize) -> Option<NodeId> {
        let window = self.scope.nearest_recursive_window()?;
        let name = window.member_names().into_iter().nth(index)?;
        self.scope.ancestors().find_map(|s| {
            if !s.bindings().pending_types().contains(&name) {
                return None;
            }
            let carried = s.nearest_recursive_window()?;
            if std::rc::Rc::ptr_eq(&carried, &window) {
                s.bindings().type_placeholder_producer(&name)
            } else {
                None
            }
        })
    }

    /// The in-flight producer of the scope that declared a SIG / abstract slot: find
    /// that scope by id, park iff it holds `name` in `pending_types`.
    fn declared_producer(&self, scope_id: ScopeId, name: &str) -> Option<NodeId> {
        let owner = self.scope.ancestors().find(|s| s.id == scope_id)?;
        if !owner.bindings().pending_types().contains(name) {
            return None;
        }
        owner.bindings().type_placeholder_producer(name)
    }
}

/// A top-level user-type reference in a type, as the finalize gate consumes it.
enum UserTypeRef {
    /// A still-in-flight nominal member, named relative to the ambient declaration window.
    Sibling { index: usize },
    /// An abstract slot, identified by its declaring scope id.
    Declared { scope_id: ScopeId, name: String },
}

/// Every top-level [`UserTypeRef`] in `kt`.
///
/// **Member discipline** (load-bearing): a sealed member node is a leaf — the walk does NOT descend
/// its schema, which holds absolute handles and may be cyclic. A sealed member is finished by
/// definition, so it is not a dependency at all; only a relative `Sibling` names something still in
/// flight, and its own schema's references are its binder's concern.
///
/// A `Signature` is a leaf too: the node carries no binder and no label, so two textually
/// identical declarations are one type and there is no declaration for a consumer to park on.
fn user_type_refs(kt: KType, types: &TypeRegistry) -> Vec<UserTypeRef> {
    let mut found = Vec::new();
    let mut stack = vec![kt];
    while let Some(handle) = stack.pop() {
        match types.node(handle) {
            TypeNode::Sibling(index) => found.push(UserTypeRef::Sibling { index }),
            TypeNode::AbstractType { source, name, .. } => found.push(UserTypeRef::Declared {
                scope_id: source,
                name,
            }),
            TypeNode::List { element } => stack.push(element),
            TypeNode::Dict { key, value } => {
                stack.push(value);
                stack.push(key);
            }
            TypeNode::Record { fields } => stack.extend(fields.values().copied()),
            TypeNode::KFunction { params, ret } => {
                stack.push(ret);
                stack.extend(params.values().copied());
            }
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                stack.extend(arguments.values().rev().copied());
                stack.push(constructor);
            }
            TypeNode::Union { members } => stack.extend(members.into_iter().rev()),
            TypeNode::Group { members } => stack.extend(members.into_iter().rev()),
            // Leaves: no nested handle. `DeferredReturn` carries only a hashable surface shadow,
            // and `Sibling` is relative content that never escapes its window.
            _ => {}
        }
    }
    found
}

#[cfg(test)]
mod tests;
