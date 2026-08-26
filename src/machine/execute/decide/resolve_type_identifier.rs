//! Scope-bound resolution of a surface type name's [`TypeSymbol`] into an interned `KType` handle —
//! Layer 2 of [design/typing/elaboration.md](../../../../design/typing/elaboration.md).
//!
//! Read-only consumer of the bindings façade: writes nothing, and of the tables reads only
//! `types` — bound identities through the elaborator, claims through the finalize gate. That
//! read-only dependency is what keeps this out of the rest of `Scope`.
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
//! A co-declared nominal still in flight never reaches this gate at all: the elaborator answers a
//! consumer's reference to an unsealed announced member with a park of its own
//! ([`elaborate_type_identifier`](crate::machine::model::elaborate_type_identifier)), and a
//! declarator's reference stays inside its own window. What is left for the gate is the
//! **declared** reference — a SIG-declared or abstract slot, identified by the declaring scope id
//! its node records.

use crate::machine::ProducerId;
use crate::machine::core::{LexicalFrame, Scope, ScopeId};
use crate::machine::model::labels::TypeSymbol;
use crate::machine::model::{KType, RunRegistries, TypeNode, TypeRegistry, TypeResolution};

impl<'step> Scope<'step> {
    /// Elaborates against `self` and admits the result only when `FinalizeGate` passes it. The
    /// Park arm — elaborator-parked or gate-rejected — is what keeps a mid-window consumer from
    /// observing pre-seal opaque identity.
    pub fn resolve_type_identifier(
        &self,
        te: TypeSymbol,
        chain: Option<std::rc::Rc<LexicalFrame>>,
        registries: &RunRegistries,
    ) -> TypeResolution<KType> {
        use crate::machine::model::{Elaborator, elaborate_type_identifier};
        let types = &registries.types;
        let mut elaborator = Elaborator::new(self).with_chain(chain);
        elaborate_type_identifier(&mut elaborator, te, registries).and_then_done(|kt| {
            let pending = FinalizeGate { scope: self, types }.pending_sources(kt);
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
/// (no type-side placeholder left there); otherwise returns the binder [`ProducerId`]s the caller
/// parks on.
///
/// The probe reads the type placeholder straight from the `types` table — not via `lookup_type`,
/// which prefers a bound arm this gate must look past to find the in-flight producer.
struct FinalizeGate<'view, 'step> {
    scope: &'view Scope<'step>,
    types: &'view TypeRegistry,
}

impl FinalizeGate<'_, '_> {
    /// Empty iff the gate admits.
    fn pending_sources(&self, kt: KType) -> Vec<ProducerId> {
        let mut pending: Vec<ProducerId> = Vec::new();
        for_each_user_type_ref(kt, self.types, &mut |UserTypeRef { scope_id, name }| {
            if let Some(node_id) = self.declared_source(scope_id, name)
                && !pending.contains(&node_id)
            {
                pending.push(node_id);
            }
        });
        pending
    }

    /// The in-flight claim edge of the scope that declared a SIG / abstract slot: find
    /// that scope by id, park iff it still holds a type placeholder for `name`.
    fn declared_source(
        &self,
        scope_id: ScopeId,
        name: crate::machine::model::TypeSymbol,
    ) -> Option<ProducerId> {
        let owner = self.scope.ancestors().find(|s| s.id == scope_id)?;
        owner.bindings().type_placeholder_producer(name)
    }
}

/// A top-level user-type reference in a type, as the finalize gate consumes it: an abstract slot,
/// identified by its declaring scope id.
struct UserTypeRef {
    scope_id: ScopeId,
    name: crate::machine::model::TypeSymbol,
}

/// Visits every top-level [`UserTypeRef`] in `kt` in pre-order, calling `found` on each. The walk
/// recurses over child handles rather than staging them in a container, so a type-annotation read
/// allocates nothing.
///
/// **Member discipline** (load-bearing): a sealed `SetMember` is a leaf — the walk does NOT descend
/// its schema, which holds absolute handles and may be cyclic. A sealed member is finished by
/// definition, so it is not a dependency at all; only a relative `Sibling` names something still in
/// flight, and its own schema's references are its binder's concern.
///
/// A `Signature` is a leaf too: the node carries no binder and no label, so two textually
/// identical declarations are one type and there is no declaration for a consumer to park on.
fn for_each_user_type_ref(kt: KType, types: &TypeRegistry, found: &mut impl FnMut(UserTypeRef)) {
    match types.node(kt) {
        TypeNode::AbstractType { source, name, .. } => found(UserTypeRef {
            scope_id: source,
            name,
        }),
        TypeNode::List { element } => for_each_user_type_ref(element, types, found),
        TypeNode::Dict { key, value } => {
            for_each_user_type_ref(key, types, found);
            for_each_user_type_ref(value, types, found);
        }
        TypeNode::Record { fields } => {
            for field in fields.values() {
                for_each_user_type_ref(*field, types, found);
            }
        }
        TypeNode::KFunction { params, ret } => {
            for param in params.values() {
                for_each_user_type_ref(*param, types, found);
            }
            for_each_user_type_ref(ret, types, found);
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            for_each_user_type_ref(constructor, types, found);
            for argument in arguments.values() {
                for_each_user_type_ref(*argument, types, found);
            }
        }
        TypeNode::Union { members } => {
            for member in members {
                for_each_user_type_ref(member, types, found);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
