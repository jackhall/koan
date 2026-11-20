//! A module's **self-signature**: a value slot per value binder at its value's type, a manifest
//! member per type binder at the handle it holds, and a keyworded member per registration at the
//! expression shape its function registers.
//!
//! One builder, [`module_signature`], serves both ends, and a `SIG` declaration besides: it is the
//! closed rule [`sealed_signature`] over the body's skeleton ([`module_signing`]). The run reads it off the activation its
//! body ran in ([`self_signature`]), each value slot at the memo its value already carries; the
//! load's walk hands it each binder's exact static type, where the load
//! knows every one. So the load's exact signature is the very handle the run computes. A
//! registration's shape is the one its function was born with, which only the function layer
//! reads, so the caller hands those in. A module's signature declares no head parameter: a body
//! binds every name it declares.

use crate::memory::BumpAllocator;
use crate::scope::{BodyShape, Reading, ShapeKind, Signed, Signing, Slot, sealed_signature};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, KType, Parametric};
use crate::values::{Chain, KnottedFamily, Value};

/// The self-signature of the module whose body `activation` ran, `keyworded` holding the shape each
/// of its registrations' functions registers. Every slot is bound: the caller runs the body to
/// completion and only then ties the binder.
pub fn self_signature<'graph, XF: KnottedFamily<'graph>>(
    activation: &Reading<'_, 'graph, '_, XF>,
    keyworded: &[DeclaredType<KType>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    let shape = activation.shape();
    let members = activation.slots().map(|(slot, value)| {
        let bound = match (shape.slot_name(slot), value) {
            (BinderSymbol::Type(_), Value::Type(held)) => DeclaredType::Type(held.handle().into()),
            (BinderSymbol::Type(_), _) => unreachable!("a type binder's slot holds a type value"),
            _ => value.ktype().into(),
        };
        (slot, bound)
    });
    module_signature(shape, members, keyworded, types, scratch)
}

/// The signature of a module whose body is `shape`: each slot of `members` a value slot at its
/// type — a quantified callable's scheme included — or, for a type binder, a manifest member at
/// the handle it holds; `keyworded` each registration's shape. A registration's slot names no
/// member, so `members` may hold it or not. A `GROUP` body's chaining is part of what it is.
pub fn module_signature(
    shape: &BodyShape<'_>,
    members: impl IntoIterator<Item = (Slot, DeclaredType<Parametric>)>,
    keyworded: &[DeclaredType<KType>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    let keyworded = keyworded
        .iter()
        .map(|shape| DeclaredType::<Parametric>::from(*shape));
    let (signed, operands) = module_signing(shape, members, keyworded, scratch);
    sealed_signature(types, scratch, &signed, operands)
}

/// The skeleton of [`module_signature`] over `members` and `keyworded`, each written over names,
/// beside the types it reads: what a binding seals as a fresh load's module signature.
pub fn module_signing<'x>(
    shape: &BodyShape<'_>,
    members: impl IntoIterator<Item = (Slot, DeclaredType<Parametric>)>,
    keyworded: impl IntoIterator<Item = DeclaredType<Parametric>>,
    scratch: BumpAllocator<'x>,
) -> (Signed<'x>, &'x [DeclaredType<Parametric>]) {
    debug_assert_eq!(shape.kind(), ShapeKind::Module);
    let mut draft = Signing::module(scratch);
    for (slot, bound) in members {
        match (shape.slot_name(slot), bound) {
            (BinderSymbol::Value(name), bound) => draft.insert_value_slot(name, bound),
            (BinderSymbol::Type(name), DeclaredType::Type(held)) => {
                draft.insert_manifest(name, held);
            }
            (BinderSymbol::Type(_), DeclaredType::Scheme(_)) => {
                unreachable!("a type binder holds a type")
            }
            // A registration names no member: its keyworded member is a shape, handed in.
            (BinderSymbol::Registration(_), _) => {}
            (BinderSymbol::Key(_), _) => unreachable!("no binder declares a key"),
        }
    }
    for shape in keyworded {
        draft.push_keyworded(shape);
    }
    // A `GROUP` body holds the group it declares, and that chaining is part of what the module is:
    // a signature stating the same group is what it satisfies. A `MODULE` holds none.
    for group in shape.held_groups() {
        draft.push_operator_group(group.members, group.mode);
    }
    draft.finished()
}
