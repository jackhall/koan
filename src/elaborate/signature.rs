//! A callable's type, read off the form node its body sits in: a `FN`'s function type over its
//! parameter schema, an `EXPR`'s or `OP`'s expression shape over its head.

use crate::memory::BumpAllocator;
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::binder::symbol_from_quote_body;
use crate::parse::builtin_shapes::role::{BodyKind, Role};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{ActivationView, Site, is_equal, is_unequal};
use crate::type_lattice::{DispatchTokenElement, KType, TypeRegistry};
use crate::values::KnottedFamily;

use super::Elaboration;
use super::expression::{Elaborator, Groups, quantifiers};

/// The type of the callable whose body sits in `form`, its signature's names read through
/// `reader` — the activation the form runs in.
///
/// A `FN` is the function type over its `:{…}` schema's fields and its return. An `EXPR` is the
/// expression shape over its head's keywords and typed slots and its return, quantified over its
/// `FOR ALL` names. A binary `OP` is the shape `operand <symbol> operand`, returning its declared
/// result or else its operand, since a chain of it folds; a `UNARY OP` is the shape `<symbol>
/// operands`, over a list of its operand, since its body takes the whole run.
pub fn callable_type<'graph, XF: KnottedFamily<'graph>>(
    form: &KExpression<'graph>,
    reader: &ActivationView<'graph, '_, XF>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Elaboration> {
    let shape = form
        .cache()
        .builtin_shape()
        .expect("a callable's body sits in a builtin shape");
    let elaborator = Elaborator {
        reader,
        types,
        scratch,
        fellows: &[],
        locals: &[],
    };
    let top = Groups {
        names: &[],
        outer: None,
    };
    let mut body = None;
    let mut signature = None;
    let mut group = None;
    let mut symbol = None;
    let mut type_parts = [None; 2];
    let mut type_count = 0;
    for (role, part) in shape.roles().zip(form.parts) {
        let part = &part.value;
        match role {
            Role::Body(kind) => body = Some((kind, part)),
            Role::Signature => signature = Some(part),
            Role::Quantifiers => group = Some(part),
            Role::Data => symbol = Some(part),
            Role::TypeExpression => {
                type_parts[type_count] = Some(part);
                type_count += 1;
            }
            _ => {}
        }
    }
    let (kind, body) = body.expect("a callable's form has a body");
    let unsupported = Elaboration::Unsupported {
        site: Site::of(body),
    };
    match kind {
        BodyKind::Lambda => {
            let (Some(signature), Some(ret)) = (signature, type_parts[0]) else {
                return Err(unsupported);
            };
            if shape.id == BuiltinShapeId::Lambda {
                return Ok(elaborator.function(&[], signature, ret, &top)?.handle);
            }
            let names = group.map(|group| quantifiers(group, scratch));
            elaborator.shape(names.as_deref().unwrap_or(&[]), signature, ret, &top)
        }
        BodyKind::Operator | BodyKind::UnaryOperator => {
            let (Some(symbol), Some(operand)) = (symbol, type_parts[0]) else {
                return Err(unsupported);
            };
            operator_shape(
                &elaborator,
                kind == BodyKind::UnaryOperator,
                symbol,
                operand,
                type_parts[1],
                &top,
            )
        }
        BodyKind::Module | BodyKind::Surfaced => Err(unsupported),
    }
}

/// The expression shape an operator head declares: `operand <symbol> operand` for a binary `OP`,
/// returning its declared result or else its operand since a chain of it folds, and `<symbol>
/// operands` over a list of its operand for a `UNARY OP`, since its body takes the whole run.
///
/// One builder, two callers: a definition reads it off the form its body sits in, and a `SIG`
/// body's bodyless head off its own parts, so a head and the definition satisfying it can never
/// spell different shapes. A run of operators chains through the signature's operator channel,
/// which a bodyless `GROUP` head fills and this builder does not write — see
/// [operator groups](../scope/README.md#operator-groups).
pub(super) fn operator_shape<'graph, XF: KnottedFamily<'graph>>(
    elaborator: &Elaborator<'_, '_, 'graph, '_, '_, XF>,
    unary: bool,
    symbol: &ExpressionPart<'graph>,
    operand: &ExpressionPart<'graph>,
    ret: Option<&ExpressionPart<'graph>>,
    groups: &Groups<'_>,
) -> Result<KType, Elaboration> {
    let unsupported = Elaboration::Unsupported {
        site: Site::of(symbol),
    };
    let ExpressionPart::QuotedExpression(quoted) = symbol else {
        return Err(unsupported);
    };
    let symbol = symbol_from_quote_body(quoted.reference()).map_err(|_| unsupported)?;
    // `!=` is nobody's to declare: every infix `a != b` is rewritten to `NOT (a == b)` before it
    // reaches a bucket, so a declaration of it would answer no call.
    if is_unequal(symbol) {
        return Err(unsupported);
    }
    let operand = elaborator.part(operand, groups)?;
    let ret = match ret {
        Some(ret) => elaborator.part(ret, groups)?,
        None if !unary => operand,
        None => return Err(unsupported),
    };
    // A result-less `OP #(==) OVER Foo` defaults its result to `Foo`, and so is refused here too:
    // the legal spelling states `-> Bool`.
    if is_equal(symbol) && ret != KType::BOOL {
        return Err(unsupported);
    }
    let (binary, run);
    let elements: &[DispatchTokenElement] = if unary {
        run = [
            DispatchTokenElement::Keyword(symbol),
            DispatchTokenElement::Slot(elaborator.types.list(operand)),
        ];
        &run
    } else {
        binary = [
            DispatchTokenElement::Slot(operand),
            DispatchTokenElement::Keyword(symbol),
            DispatchTokenElement::Slot(operand),
        ];
        &binary
    };
    Ok(elaborator
        .types
        .shape_type(elaborator.scratch, &[], elements, ret)
        .handle)
}
