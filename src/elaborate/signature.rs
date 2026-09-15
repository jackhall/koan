//! A callable's type, read off the form node its body sits in: a `FN`'s function type over its
//! parameter schema, an `EXPR`'s or `OP`'s expression shape over its head.

use crate::memory::BumpAllocator;
use crate::parse::forms::FormId;
use crate::parse::forms::binder::symbol_from_quote_body;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{Activation, BodyKind, Role, Site, roles};
use crate::type_lattice::{DispatchTokenElement, KType, TypeRegistry};
use crate::values::Callable;

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
pub fn callable_type<'graph, X: Callable>(
    form: &KExpression<'graph>,
    reader: &Activation<'graph, '_, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Elaboration> {
    let id = form
        .cache()
        .form()
        .expect("a callable's body sits in a builtin form")
        .id;
    let elaborator = Elaborator {
        reader,
        types,
        scratch,
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
    for (role, part) in roles(id).iter().zip(form.parts) {
        let part = &part.value;
        match role {
            Role::Body(kind) => body = Some((*kind, part)),
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
            if id == FormId::Lambda {
                return elaborator.function(signature, ret, &top);
            }
            let names = group.map(|group| quantifiers(group, scratch));
            elaborator.shape(names.as_deref().unwrap_or(&[]), signature, ret, &top)
        }
        BodyKind::Operator | BodyKind::UnaryOperator => {
            let (Some(symbol), Some(operand)) = (symbol, type_parts[0]) else {
                return Err(unsupported);
            };
            let ExpressionPart::QuotedExpression(quoted) = symbol else {
                return Err(Elaboration::Unsupported {
                    site: Site::of(symbol),
                });
            };
            let symbol = symbol_from_quote_body(quoted.reference()).map_err(|_| {
                Elaboration::Unsupported {
                    site: Site::of(symbol),
                }
            })?;
            let operand = elaborator.part(operand, &top)?;
            let ret = match type_parts[1] {
                Some(ret) => elaborator.part(ret, &top)?,
                None if kind == BodyKind::Operator => operand,
                None => return Err(unsupported),
            };
            let (binary, unary);
            let elements: &[DispatchTokenElement] = if kind == BodyKind::Operator {
                binary = [
                    DispatchTokenElement::Slot(operand),
                    DispatchTokenElement::Keyword(symbol),
                    DispatchTokenElement::Slot(operand),
                ];
                &binary
            } else {
                unary = [
                    DispatchTokenElement::Keyword(symbol),
                    DispatchTokenElement::Slot(types.list(operand)),
                ];
                &unary
            };
            Ok(types.shape_type(scratch, &[], elements, ret).handle)
        }
        BodyKind::Module => Err(unsupported),
    }
}
