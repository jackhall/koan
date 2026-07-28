//! Scope-bound resolution of a surface [`TypeIdentifier`] into an interned `KType` handle.
//!
//! Read-only consumer of the bindings façade: writes nothing, and of the tables reads only
//! `types` (through the elaborator) and the type-side `placeholders` — the read-only dependency
//! is what justifies the split from `scope.rs`.
//!
//! ## Invariant pinned here
//!
//! **No consumer observes a not-yet-sealed type identity.** A `Done` result survives only
//! when every user-type the elaborated result references is finalized; a referenced type still
//! in flight demotes it to a `Park` on that type's producer, so a half-built identity cannot
//! reach a consumer.
//!
//! In-flight-ness *is* the type-side placeholder: stamped at the binder's submission, cleared
//! atomically with the `types` insert when its write op applies. A name carrying a placeholder in
//! some scope is a binder that has not yet installed its identity there.
//!
//! Which scope to probe is decided per reference kind. A nominal member in flight is named by a
//! relative `Sibling` handle, which is meaningful only against the **declaration window** that
//! minted it — the nearest one on this scope's chain. A member whose slot that window has already
//! filled is settled and never in flight: the group installs one identity write for every member at
//! the seal, so a filled member's placeholder outlives its own finalize. For an unfilled one the
//! gate resolves the index to a member name, then walks for the scope that both carries that same
//! window and holds a placeholder naming the producer to park on. Window identity is what the
//! ptr-equality does here: it stops an unrelated same-named
//! declaration, which opens its own window, from capturing the reference. A sealed member carries
//! an absolute handle and no window, so it is never in flight. A SIG-declared or abstract slot is
//! identified by the declaring scope id its node records.

use crate::machine::core::NodeId;
use crate::machine::core::{LexicalFrame, Scope, ScopeId};
use crate::machine::model::TypeIdentifier;
use crate::machine::model::{KType, TypeNode, TypeRegistry, TypeResolution};

impl<'step> Scope<'step> {
    /// Layer-2 scope-bound TypeIdentifier resolution: elaborates against `self` and admits
    /// the result only when a [`FinalizeGate`] passes it. The Park arm — elaborator-parked
    /// or gate-rejected — is what keeps a mid-window consumer from observing pre-seal
    /// opaque identity.
    pub fn resolve_type_identifier(
        &self,
        te: &TypeIdentifier,
        chain: Option<std::rc::Rc<LexicalFrame>>,
        types: &TypeRegistry,
    ) -> TypeResolution<KType> {
        use crate::machine::model::{elaborate_type_identifier, Elaborator};
        let mut elaborator = Elaborator::new(self).with_chain(chain);
        // A referenced type still in flight demotes this `Done` to a `Park`; `Park` /
        // `Unbound` forward unchanged.
        elaborate_type_identifier(&mut elaborator, te, types).and_then_done(|kt| {
            let pending = FinalizeGate { scope: self, types }.pending_producers(kt);
            if pending.is_empty() {
                TypeResolution::Done(kt)
            } else {
                TypeResolution::Park(pending)
            }
        })
    }
}

/// Precondition value for a resolved identity, naming the load-bearing invariant
/// *"no not-yet-sealed type may reach a consumer"* as a type.
///
/// Admits a `KType` iff every top-level user-type it references is finalized in its owning scope
/// (no type-side placeholder left there); otherwise returns the producer `NodeId`s the caller
/// parks on.
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
        // A filled slot ends the member's flight: its own finalize has run, so the relative handle
        // this reference holds already denotes settled content. The member's placeholder outlives
        // that — a group's identity write, and with it the clear, waits for the seal — so for a
        // sibling the slot is the finer signal, and the placeholder below only names the producer
        // node to park on.
        if window.member_is_filled(index) {
            return None;
        }
        let name = window.member_names().into_iter().nth(index)?;
        self.scope.ancestors().find_map(|s| {
            let carried = s.nearest_recursive_window()?;
            if std::rc::Rc::ptr_eq(&carried, &window) {
                s.bindings().type_placeholder_producer(&name)
            } else {
                None
            }
        })
    }

    /// The in-flight producer of the scope that declared a SIG / abstract slot: find
    /// that scope by id, park iff it still holds a type placeholder for `name`.
    fn declared_producer(&self, scope_id: ScopeId, name: &str) -> Option<NodeId> {
        let owner = self.scope.ancestors().find(|s| s.id == scope_id)?;
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
