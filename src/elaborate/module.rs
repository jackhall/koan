//! A module's **self-signature**, read off the activation its body ran in: a value slot per value
//! binder at the type its value carries, a manifest member per type binder at the handle it holds,
//! and a keyworded member per registration at the expression shape its function registers.
//!
//! Nothing here walks a value: a slot's type is the memo its value already carries, which the tie
//! derived, and a registration's shape is the one its function was born with, which only the
//! function layer reads — so the caller hands those in. A module's signature declares no abstract
//! member: a body binds every name it declares.

use crate::memory::BumpAllocator;
use crate::scope::{ActivationView, ShapeKind};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, SchemaDraft, TypeRegistry};
use crate::values::{KnottedFamily, Value};

/// The self-signature of the module whose body `activation` ran, `keyworded` holding the shape each
/// of its registrations' functions registers. Every slot is bound: the caller runs the body to
/// completion and only then ties the binder.
pub fn self_signature<'graph, XF: KnottedFamily<'graph>>(
    activation: &ActivationView<'graph, '_, XF>,
    keyworded: &[KType],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    debug_assert_eq!(activation.shape().kind(), ShapeKind::Module);
    let mut draft = SchemaDraft::new(scratch);
    for (slot, value) in activation.slots() {
        let name = activation.shape().slot_name(slot);
        match name {
            BinderSymbol::Value(name) => draft.insert_value_slot(name, value.ktype()),
            BinderSymbol::Type(name) => {
                let Value::Type(held) = value else {
                    unreachable!("a type binder's slot holds a type value");
                };
                draft.insert_manifest(name, held.handle());
            }
            // A registration names no member: its keyworded member is a shape, handed in.
            BinderSymbol::Registration(_) => {}
            BinderSymbol::Key(_) => unreachable!("no binder declares a key"),
        }
    }
    for shape in keyworded {
        draft.push_keyworded(*shape);
    }
    // A `GROUP` body holds the group it declares, and that chaining is part of what the module is:
    // a signature stating the same group is what it satisfies. A `MODULE` holds none.
    for group in activation.shape().held_groups() {
        draft.push_operator_group(group.members, group.mode);
    }
    types.signature(scratch, draft)
}
