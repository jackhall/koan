//! A builtin shape's overloads, interned as lattice handles, and the builtin `Result` and `Error`
//! unions: the recipe the builtin image is generated from
//! ([builtin_image.rs](../builtin_image.rs)), compiled for tests alone. A load builds none of
//! them; it reads the image's constants.
//!
//! [`BUILTIN_SHAPES`](crate::parse::BUILTIN_SHAPES) states each bucket's overloads
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
use crate::parse::{BuiltinShape, ShapeElement};
use crate::scope::{FaultKind, Field};
use crate::symbols::{BinderSymbol, StaticName, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DispatchTokenElement, KKind, KType, RecursiveGroupWindow, RelativeSchema,
};
use crate::values::Chain;

/// The handle of each overload of `shape`, in overload order. Empty for a reserved bucket, which
/// nothing registers under: its slot types exist to keep its parts raw so its miss stays a miss,
/// not to name a callable anything can reach.
pub fn builtin_shape_types<'x>(
    shape: &'static BuiltinShape,
    types: &Chain<'_, '_, '_, '_>,
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
        handles.push(types.shape_type(&elements, &[], ret));
    }
    handles.leak()
}

static RESULT: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Result");
static OK: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Ok");
static ERROR: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Error");
static MESSAGE: StaticName<ValueSymbol> = crate::static_name!(ValueSymbol, "message");

/// The builtin `Result`: the union `UNION (Ok Error -> Result) = (Ok :Ok Error :Error)` declares,
/// sealed through the window that declaration seals through, its names recorded in `symbols`.
/// Each variant is a family over both parameters wrapping the one it is named for.
pub fn builtin_result(
    types: &Chain<'_, '_, '_, '_>,
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

/// The builtin `Error`: the union a declaration listing one variant per [`FaultKind`], in
/// [`FaultKind::ALL`]'s order, each over the kind's fields beside `message` and `frames`, would
/// seal, sealed through the window that declaration seals through, its names recorded in `symbols`
/// ([catching](../dispatch/README.md#catching)).
pub fn builtin_error(
    types: &Chain<'_, '_, '_, '_>,
    symbols: &SymbolInterner,
    scratch: BumpAllocator<'_>,
) -> KType {
    let error = symbols.record(&ERROR);
    let field = |name| BinderSymbol::Value(ValueSymbol::declared(name, symbols).expect("a name"));
    let frame = types.record(
        scratch,
        &[
            (field("callee"), KType::STR),
            (field("call"), KType::STR),
            (field("at"), KType::STR),
            (field("count"), KType::NUMBER),
        ],
    );
    let frames = types.list(frame);
    let mut members = BumpVec::with_capacity_in(FaultKind::ALL.len(), scratch);
    members.extend(FaultKind::ALL.iter().map(|kind| {
        let label = TypeSymbol::declared(kind.name(), symbols).expect("a kind names a type");
        (label, Some(error), KKind::NewType)
    }));
    let owned: &[usize] = scratch.alloc_slice_fill_iter(0..members.len());
    let window = RecursiveGroupWindow::for_component(scratch, &members, &[(error, owned)]);
    let mut sealed = None;
    for (index, kind) in FaultKind::ALL.into_iter().enumerate() {
        let mut fields = BumpVec::with_capacity_in(kind.fields().len() + 2, scratch);
        fields.push((BinderSymbol::Value(symbols.record(&MESSAGE)), KType::STR));
        fields.push((field("frames"), frames));
        fields.extend(kind.fields().iter().map(|(name, declared)| {
            let declared = match declared {
                Field::Any => KType::ANY,
                Field::Str => KType::STR,
                Field::Number => KType::NUMBER,
                Field::Type => KType::of_kind(KKind::AnyType),
                Field::ListOfAny => KType::LIST_OF_ANY,
            };
            (field(name), declared)
        }));
        let payload = types.record(scratch, &fields);
        sealed = window.fill_member(index, RelativeSchema::NewType(payload), types, scratch);
    }
    sealed
        .and_then(|sealed| sealed.binder_type(error))
        .expect("the last fill seals the union")
}
