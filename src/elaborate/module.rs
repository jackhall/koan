//! A module's **self-signature**, read off the activation its body ran in: a value slot per value
//! binder at the type its value carries, and a manifest member per type binder at the handle it
//! holds.
//!
//! Nothing here walks a value: a slot's type is the memo its value already carries, which the tie
//! derived. A module's signature declares no abstract member — a body binds every name it
//! declares — and its keyworded and operator channels are empty until
//! [dispatch](../../roadmap/rewrite/dispatch.md) gives a bodyless definition a slot.

use crate::memory::{BumpAllocator, CellHandle};
use crate::scope::{Activation, Binding, ShapeKind};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, SchemaDraft, TypeRegistry};
use crate::values::{Knotted, Value};

/// Why a module's self-signature cannot be read yet: the binder of `name` is still running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsigned {
    pub name: BinderSymbol,
    pub binder: CellHandle,
}

/// The self-signature of the module whose body `activation` ran. Every slot must be bound: the
/// caller runs the body to completion and only then ties the binder.
pub fn self_signature<X: Knotted>(
    activation: &Activation<'_, '_, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Unsigned> {
    debug_assert_eq!(activation.shape().kind(), ShapeKind::Module);
    let mut draft = SchemaDraft::new(scratch);
    for (slot, binding) in activation.slots() {
        let name = activation.shape().slot_name(slot);
        let value = match binding {
            Binding::Bound(value) => value,
            Binding::Pending(binder) => return Err(Unsigned { name, binder }),
        };
        match name {
            BinderSymbol::Value(name) => draft.insert_value_slot(name, value.ktype()),
            BinderSymbol::Type(name) => {
                let Value::Type(held) = value else {
                    unreachable!("a type binder's slot holds a type value");
                };
                draft.insert_manifest(name, held.handle());
            }
        }
    }
    Ok(types.signature(scratch, draft))
}
