//! A builtin shape's overloads, interned as lattice handles.
//!
//! [`BUILTIN_SHAPES`](crate::parse::builtin_shapes::BUILTIN_SHAPES) states each bucket's overloads
//! as `static` data — a [`SlotType`] per slot per overload, and one return apiece — because the
//! parser probes the table before any registry exists. This is the one door that turns such an
//! entry into [`ExpressionShape`](crate::type_lattice::TypeNode::ExpressionShape) handles: one per
//! overload, in overload order, each erasing to the entry's own bucket key.
//!
//! Nothing here reads a name, so nothing here fails.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::{BuiltinShape, ShapeElement, SlotType};
use crate::type_lattice::{DispatchTokenElement, KType, TypeRegistry};

/// The handle of each overload of `shape`, in overload order. Empty for a reserved bucket, which
/// nothing registers under: its slot types exist to keep its parts raw so its miss stays a miss,
/// not to name a callable anything can reach.
pub fn builtin_shape_types<'x>(
    shape: &'static BuiltinShape,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> &'x [KType] {
    if shape.reserved {
        return &[];
    }
    let mut handles = BumpVec::with_capacity_in(shape.overloads(), scratch);
    for overload in 0..shape.overloads() {
        let mut elements = BumpVec::with_capacity_in(shape.elements.len(), scratch);
        for element in shape.elements {
            elements.push(match element {
                ShapeElement::Keyword(name) => DispatchTokenElement::Keyword(name.symbol()),
                ShapeElement::Slot { types: slot, .. } => {
                    DispatchTokenElement::Slot(slot_type(slot[overload], types, scratch))
                }
            });
        }
        let ret = slot_type(shape.returns[overload], types, scratch);
        handles.push(types.shape_type(scratch, &[], &elements, ret).handle);
    }
    handles.leak()
}

/// One static slot type as a handle: a leaf is already one, and the two compounds are interned
/// here, which is the whole reason this door exists.
fn slot_type(spec: SlotType, types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>) -> KType {
    match spec {
        SlotType::Leaf(handle) => handle,
        SlotType::Union(members) => types.union_of(scratch, members),
        SlotType::EmptyRecord => types.record(scratch, &[]),
    }
}
