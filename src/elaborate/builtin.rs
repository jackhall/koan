//! A builtin shape's overloads, interned as lattice handles, and the builtin `Result` family.
//!
//! [`BUILTIN_SHAPES`](crate::parse::builtin_shapes::BUILTIN_SHAPES) states each bucket's overloads
//! as `static` data — a [`SlotType`] per slot per overload, and one return apiece — because the
//! parser probes the table before any registry exists. This is the one door that turns such an
//! entry into [`ExpressionShape`](crate::type_lattice::TypeNode::ExpressionShape) handles: one per
//! overload, in overload order, each erasing to the entry's own bucket key.
//!
//! [`builtin_result`] seals the union its own declaration would, so the builtin and a declared
//! `Result` are one handle.
//!
//! Nothing here reads a name, so nothing here fails.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::{BuiltinShape, ShapeElement, SlotType};
use crate::symbols::{StaticName, SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    DispatchTokenElement, KKind, KType, RecursiveGroupWindow, RelativeSchema, TypeRegistry,
};

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

static RESULT: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Result");
static OK: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Ok");
static ERROR: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Error");

/// The builtin `Result`: the union `UNION (Ok Error AS Result) = (Ok :Ok Error :Error)` declares,
/// sealed through the window that declaration seals through, its names recorded in `symbols`.
/// Each variant is a family over both parameters wrapping the one it is named for.
pub fn builtin_result(
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
    scratch: BumpAllocator<'_>,
) -> KType {
    let (result, ok, error) = (
        symbols.record(&RESULT),
        symbols.record(&OK),
        symbols.record(&ERROR),
    );
    let mut params = [ok, error];
    params.sort_unstable();
    let parameter = |name| {
        let index = params
            .iter()
            .position(|param| *param == name)
            .expect("a declared parameter");
        types.quantified(index, KType::ANY)
    };
    let window = RecursiveGroupWindow::for_component(
        scratch,
        &[
            (ok, Some(result), KKind::TypeConstructor),
            (error, Some(result), KKind::TypeConstructor),
        ],
        &[(result, &[0, 1])],
    );
    for (index, variant) in [ok, error].into_iter().enumerate() {
        let schema =
            RelativeSchema::constructor(scratch, scratch, Some(parameter(variant)), &params);
        window.fill_member(index, schema, types, scratch);
    }
    window
        .sealed()
        .and_then(|sealed| sealed.binder_type(result))
        .expect("the last fill seals the union")
}
