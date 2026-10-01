//! A builtin shape's overloads, interned as lattice handles, the builtin `Result` family, and the
//! builtin `Error` nominal a koan error value is tagged with.
//!
//! [`BUILTIN_SHAPES`](crate::parse::builtin_shapes::BUILTIN_SHAPES) states each bucket's overloads
//! as `static` data — a `const` [`KType`] per slot per overload, and one return apiece — because
//! the parser probes the table before any registry exists. This is the one door that assembles
//! such an entry's handles into [`ExpressionShape`](crate::type_lattice::TypeNode::ExpressionShape)
//! handles: one per overload, in overload order, each erasing to the entry's own bucket key.
//!
//! [`builtin_result`] and [`builtin_error`] each seal what their own declaration would, so a builtin
//! and its declared twin are one handle.
//!
//! Nothing here reads a name, so nothing here fails.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::{BuiltinShape, ShapeElement};
use crate::symbols::{BinderSymbol, StaticName, SymbolInterner, TypeSymbol, ValueSymbol};
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
                    DispatchTokenElement::Slot(slot[overload])
                }
            });
        }
        let ret = shape.returns[overload];
        handles.push(
            types
                .shape_type(scratch, &[], &[], &elements, &[], ret)
                .handle,
        );
    }
    handles.leak()
}

static RESULT: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Result");
static OK: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Ok");
static ERROR: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Error");
static MESSAGE: StaticName<ValueSymbol> = crate::static_name!(ValueSymbol, "message");

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

/// The builtin `Error`: the nominal `NEWTYPE Error = :{message :Str}` declares, sealed through the
/// window that declaration seals through, its names recorded in `symbols`. A koan error value is a
/// tagged value of it.
pub fn builtin_error(
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
    scratch: BumpAllocator<'_>,
) -> KType {
    let error = symbols.record(&ERROR);
    let message = BinderSymbol::Value(symbols.record(&MESSAGE));
    let payload = types.record(scratch, &[(message, KType::STR)]);
    let window =
        RecursiveGroupWindow::for_component(scratch, &[(error, None, KKind::NewType)], &[]);
    window
        .fill_member(0, RelativeSchema::NewType(payload), types, scratch)
        .and_then(|sealed| sealed.member(0))
        .expect("the only fill seals the newtype")
}
