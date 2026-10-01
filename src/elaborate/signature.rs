//! A callable's type, read off the builtin shape node its body sits in: always a function type (a
//! `FN`'s over its parameter schema, an `EXPR` definition's over its head's slot names, an
//! operator's over `left` and `right` or `operands`), and, for the function a registration binds,
//! what its bucket holds: the expression shape built from that function type over the
//! registration's key and ranked by its classes, and how a keyworded call binds the shape's slots to
//! the function's parameters. A call is by name through the function type; only a bucket reads the
//! shape. See [dispatch](../dispatch/README.md).

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::binder::symbol_from_quote_body;
use crate::parse::builtin_shapes::role::{BodyKind, Role};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    Callable, Elaboration, IMPLICIT, ParameterBinding, Registered, Registration, Site, Which,
    is_equal, is_unequal,
};
use crate::symbols::{BinderSymbol, KeywordSymbol, Symbol, TypeSymbol};
use crate::type_lattice::{DispatchTokenElement, GroupIntern, KType, TypeNode, TypeRegistry};

use super::expression::{Elaborator, Groups, HeadElement, QuantifierGroup, walk_head};
use super::reads::Reads;

/// The type of the callable whose body sits in `form`, its signature's names read through
/// `reader` — the activation the form runs in — born for `registration`, or for no registration.
///
/// The type is always a function type: a `FN`'s over its parameter schema, an `EXPR` definition's
/// (bare or combined) over its head's slot names, quantified where the form carries a `FOR ALL`
/// group, a binary `OP`'s over `left` and `right`, and a `UNARY OP`'s over `operands`, a list of
/// its operand. Born for a registration, it also hands back what the registration's bucket holds.
pub fn callable_type<'graph, 'x, R: Reads<'graph> + ?Sized>(
    form: &KExpression<'graph>,
    reader: &R,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    registration: Option<&Registration<'_>>,
) -> Result<Callable<'x>, Elaboration> {
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
        binder: Cell::new(false),
    };
    let top = Groups {
        names: &[],
        bounds: &[],
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
            Role::Signature | Role::Head => signature = Some(part),
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
            let group = match group {
                Some(part) => elaborator.group(part, &top)?,
                None => QuantifierGroup::empty(scratch),
            };
            let (interned, head) = match shape.id {
                BuiltinShapeId::Lambda | BuiltinShapeId::QuantifiedLambda => {
                    (elaborator.function(&group, signature, ret, &top)?, None)
                }
                BuiltinShapeId::ExpressionDefinition
                | BuiltinShapeId::QuantifiedExpressionDefinition
                | BuiltinShapeId::CombinedExpression
                | BuiltinShapeId::CombinedQuantifiedExpression => {
                    let interned = elaborator.head_function(&group, signature, ret, &top)?;
                    let ExpressionPart::QuotedExpression(run) = signature else {
                        unreachable!("`head_function` refused a head that is no quote")
                    };
                    (interned, Some(Head::Written(run.reference())))
                }
                _ => return Err(unsupported),
            };
            let mut map = BumpVec::with_capacity_in(group.names.len(), scratch);
            map.extend(
                group
                    .names
                    .iter()
                    .zip(interned.quantifier_map)
                    .map(|(name, index)| (*name, *index)),
            );
            let map = map.leak();
            let registered = registration.map(|registration| {
                let head = head.expect("only an `EXPR` definition registers a lambda body");
                registered(
                    types,
                    scratch,
                    interned.handle,
                    map,
                    head,
                    registration.classes,
                )
            });
            Ok(Callable {
                ktype: interned.handle,
                quantifier_map: map,
                registered,
            })
        }
        BodyKind::Operator | BodyKind::UnaryOperator => {
            let (Some(symbol), Some(operand)) = (symbol, type_parts[0]) else {
                return Err(unsupported);
            };
            let unary = kind == BodyKind::UnaryOperator;
            let (symbol, function, operand) =
                operator_function(&elaborator, unary, symbol, operand, type_parts[1], &top)?;
            let registered = registration.map(|registration| {
                let head = match registration.which {
                    Which::Only | Which::Unary => Head::operator(unary, symbol),
                    Which::Binary => Head::Bridge(symbol, operand),
                };
                registered(types, scratch, function, &[], head, registration.classes)
            });
            Ok(Callable {
                ktype: function,
                quantifier_map: &[],
                registered,
            })
        }
        BodyKind::Module | BodyKind::Surfaced => Err(unsupported),
    }
}

/// The expression shape an operator head declares: the one [`registered_shape`] builds from
/// [`operator_function`]'s type over `operand <symbol> operand` or `<symbol> operands`.
///
/// One builder, two callers: a definition registers the shape built from its function type, and a
/// `SIG` body's bodyless head is built by this same door, so a head and the definition satisfying
/// it can never spell different shapes. A run of operators chains through the signature's operator
/// channel, which a bodyless `GROUP` head fills and this builder does not write — see
/// [operator groups](../scope/README.md#operator-groups).
pub(super) fn operator_shape<'graph, R: Reads<'graph> + ?Sized>(
    elaborator: &Elaborator<'_, '_, '_, R>,
    unary: bool,
    symbol: &ExpressionPart<'graph>,
    operand: &ExpressionPart<'graph>,
    ret: Option<&ExpressionPart<'graph>>,
    groups: &Groups<'_>,
) -> Result<KType, Elaboration> {
    let (symbol, function, _) = operator_function(elaborator, unary, symbol, operand, ret, groups)?;
    let (shape, _) = registered_shape(
        elaborator.types,
        elaborator.scratch,
        function,
        Head::operator(unary, symbol),
        &[],
    );
    Ok(shape.handle)
}

/// The function type an operator head declares, beside its symbol and its operand type: `FN
/// :{left :Operand, right :Operand} -> Ret` for a binary `OP`, returning its declared result or
/// else its operand since a chain of it folds, and `FN :{operands :(LIST OF Operand)} -> Ret` for a
/// `UNARY OP`, since its body takes the whole run. The parameter names are the ones the shape
/// builder binds in the operator's body.
fn operator_function<'graph, R: Reads<'graph> + ?Sized>(
    elaborator: &Elaborator<'_, '_, '_, R>,
    unary: bool,
    symbol: &ExpressionPart<'graph>,
    operand: &ExpressionPart<'graph>,
    ret: Option<&ExpressionPart<'graph>>,
    groups: &Groups<'_>,
) -> Result<(KeywordSymbol, KType, KType), Elaboration> {
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
    let (types, scratch) = (elaborator.types, elaborator.scratch);
    let function = if unary {
        let operands = BinderSymbol::Value(IMPLICIT.operands.symbol());
        types.function_type(scratch, &[], &[], &[(operands, types.list(operand))], ret)
    } else {
        let left = BinderSymbol::Value(IMPLICIT.left.symbol());
        let right = BinderSymbol::Value(IMPLICIT.right.symbol());
        types.function_type(scratch, &[], &[], &[(left, operand), (right, operand)], ret)
    };
    Ok((symbol, function.handle, operand))
}

/// Where a registration's keywords and slots sit, in written order.
#[derive(Clone, Copy)]
enum Head<'h, 'graph> {
    /// An `EXPR` head's run: each keyword where it is written, each `<name> :<Type>` pair a slot.
    Written(&'h KExpression<'graph>),
    /// `left <symbol> right`.
    Binary(KeywordSymbol),
    /// `<symbol> operands`.
    Unary(KeywordSymbol),
    /// A `UNARY OP`'s binary key, `<operand> <symbol> <operand>`, whose slots a call packs into
    /// `operands`.
    Bridge(KeywordSymbol, KType),
}

impl Head<'_, '_> {
    fn operator(unary: bool, symbol: KeywordSymbol) -> Self {
        if unary {
            Head::Unary(symbol)
        } else {
            Head::Binary(symbol)
        }
    }
}

/// What a registration's bucket holds of `function`, the definition's function type, whose
/// declaration-order quantifier map is `map`: the shape [`registered_shape`] builds over `head`
/// ranked by `classes`, `map` read through into that shape's group, and the parameter binding.
fn registered<'x>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    function: KType,
    map: &[(TypeSymbol, usize)],
    head: Head<'_, '_>,
    classes: &[u8],
) -> Registered<'x> {
    let (shape, parameters) = registered_shape(types, scratch, function, head, classes);
    // The shape's group is the function's, renumbered.
    let mut read_through = BumpVec::with_capacity_in(map.len(), scratch);
    read_through.extend(
        map.iter()
            .map(|(name, index)| (*name, shape.quantifier_map[*index])),
    );
    Registered {
        shape: shape.handle,
        quantifier_map: read_through.leak(),
        parameters,
    }
}

/// The expression shape a registration puts in its bucket, built from `function`, the definition's
/// function type, over `head`: each keyword where the head writes it, and each slot at the type
/// `function` gives the parameter it names, ranked by `classes`, over `function`'s return and
/// quantified over its group — beside how a call binds the slots to the parameters.
///
/// The shape door renumbers that group by first occurrence in element order, so the shape is the
/// one `:(EXPR …)` spells for the same head, although `function` numbered it in its parameters'
/// sorted order; the returned map translates `function`'s group into the shape's.
fn registered_shape<'x>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    function: KType,
    head: Head<'_, '_>,
    classes: &[u8],
) -> (GroupIntern<'x>, ParameterBinding<'x>) {
    let TypeNode::KFunction {
        quantifiers,
        bounds,
        params,
        ret,
    } = types.node(function)
    else {
        unreachable!("a definition is typed by its function type")
    };
    let slot = |name: Symbol| {
        DispatchTokenElement::Slot(
            params
                .get(name)
                .expect("every slot a head names is one of its parameters"),
        )
    };
    let (left, right, operands) = (
        BinderSymbol::Value(IMPLICIT.left.symbol()),
        BinderSymbol::Value(IMPLICIT.right.symbol()),
        BinderSymbol::Value(IMPLICIT.operands.symbol()),
    );
    let mut elements = BumpVec::new_in(scratch);
    let parameters = match head {
        Head::Written(run) => {
            let mut names = BumpVec::new_in(scratch);
            walk_head(run, (), |element| {
                elements.push(match element {
                    HeadElement::Keyword(symbol) => DispatchTokenElement::Keyword(symbol),
                    HeadElement::Slot(label, _) => {
                        let name = label
                            .name()
                            .expect("`head_function` refused a nameless slot");
                        names.push(name);
                        slot(name.symbol())
                    }
                });
                Ok(())
            })
            .expect("`head_function` refused every other part");
            ParameterBinding::Named(names.leak())
        }
        Head::Binary(symbol) => {
            elements.extend([
                slot(left.symbol()),
                DispatchTokenElement::Keyword(symbol),
                slot(right.symbol()),
            ]);
            ParameterBinding::Named(scratch.alloc_slice_copy(&[left, right]))
        }
        Head::Unary(symbol) => {
            elements.extend([
                DispatchTokenElement::Keyword(symbol),
                slot(operands.symbol()),
            ]);
            ParameterBinding::Named(scratch.alloc_slice_copy(&[operands]))
        }
        Head::Bridge(symbol, operand) => {
            elements.extend([
                DispatchTokenElement::Slot(operand),
                DispatchTokenElement::Keyword(symbol),
                DispatchTokenElement::Slot(operand),
            ]);
            ParameterBinding::Operands
        }
    };
    let shape = types.shape_type(scratch, quantifiers, bounds, &elements, classes, ret);
    (shape, parameters)
}
