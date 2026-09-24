//! A callable's type, read off the form node its body sits in: always a function type (a `FN`'s
//! over its parameter schema, an `EXPR` definition's over its head's slot names, an operator's over
//! `left` and `right` or `operands`), and, for a definition that registers, the expression shape
//! its bucket holds, built once from that function type over the head. A call is by name through
//! the function type; only a bucket reads the shape. See
//! [dispatch](../../roadmap/rewrite/dispatch.md).

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::binder::symbol_from_quote_body;
use crate::parse::builtin_shapes::role::{BodyKind, Role};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{ActivationView, IMPLICIT, Site, is_equal, is_unequal};
use crate::symbols::{BinderSymbol, KeywordSymbol, Symbol, TypeSymbol};
use crate::type_lattice::{DispatchTokenElement, KType, TypeNode, TypeRegistry};
use crate::values::KnottedFamily;

use super::Elaboration;
use super::expression::{Elaborator, Groups, HeadElement, QuantifierGroup, walk_head};

/// Where a `FOR ALL` name the declaration wrote landed in its callable's canonical group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Canonical {
    /// The group's variable at this index: a call binds the name to what the group solves it to.
    At(usize),
    /// Dropped by canonical form: a call binds the name to its bound.
    Dropped { bound: KType },
}

/// A callable's type, and how its `FOR ALL` group's declaration order maps onto that type's
/// canonical group — what a call needs to bind each type parameter to its solution.
pub struct Callable<'x> {
    pub ktype: KType,
    /// Each `FOR ALL` name the declaration wrote, in written order, with where it landed in
    /// `ktype`'s canonical group. Empty for an unquantified callable.
    ///
    /// The **name** is the key, not the position: a callee's type-parameter slots reach its frame
    /// symbol-sorted, not in written order, and `ktype`'s own `quantifiers` cannot stand in for
    /// this because alpha-variants intern to one node and it holds whichever spelling interned
    /// first.
    pub quantifier_map: &'x [(TypeSymbol, Canonical)],
    /// The expression shape the definition's registration puts in its bucket, built from `ktype`
    /// over the head where the callable is born; `None` for a `FN`, which no bucket holds.
    pub registered: Option<KType>,
}

/// The type of the callable whose body sits in `form`, its signature's names read through
/// `reader` — the activation the form runs in.
///
/// The type is always a function type: a `FN`'s over its parameter schema, an `EXPR` definition's
/// (bare or combined) over its head's slot names, quantified where the form carries a `FOR ALL`
/// group, a binary `OP`'s over `left` and `right`, and a `UNARY OP`'s over `operands`, a list of
/// its operand. A definition that registers, every `EXPR` and operator one, also hands back the
/// shape its bucket holds, built from that function type over its head.
pub fn callable_type<'graph, 'x, XF: KnottedFamily<'graph>>(
    form: &KExpression<'graph>,
    reader: &ActivationView<'graph, '_, XF>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
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
                    let ExpressionPart::Expression(run) = signature else {
                        unreachable!("`head_function` refused a head that is no run")
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
                    .zip(&group.bounds)
                    .zip(interned.quantifier_map)
                    .map(|((name, bound), canonical)| {
                        let canonical = match canonical {
                            Some(index) => Canonical::At(*index),
                            None => Canonical::Dropped { bound: *bound },
                        };
                        (*name, canonical)
                    }),
            );
            Ok(Callable {
                ktype: interned.handle,
                quantifier_map: map.leak(),
                registered: head
                    .map(|head| registered_shape(types, scratch, interned.handle, head)),
            })
        }
        BodyKind::Operator | BodyKind::UnaryOperator => {
            let (Some(symbol), Some(operand)) = (symbol, type_parts[0]) else {
                return Err(unsupported);
            };
            let unary = kind == BodyKind::UnaryOperator;
            let (symbol, function) =
                operator_function(&elaborator, unary, symbol, operand, type_parts[1], &top)?;
            Ok(Callable {
                ktype: function,
                quantifier_map: &[],
                registered: Some(registered_shape(
                    types,
                    scratch,
                    function,
                    Head::operator(unary, symbol),
                )),
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
pub(super) fn operator_shape<'graph, XF: KnottedFamily<'graph>>(
    elaborator: &Elaborator<'_, '_, 'graph, '_, '_, XF>,
    unary: bool,
    symbol: &ExpressionPart<'graph>,
    operand: &ExpressionPart<'graph>,
    ret: Option<&ExpressionPart<'graph>>,
    groups: &Groups<'_>,
) -> Result<KType, Elaboration> {
    let (symbol, function) = operator_function(elaborator, unary, symbol, operand, ret, groups)?;
    Ok(registered_shape(
        elaborator.types,
        elaborator.scratch,
        function,
        Head::operator(unary, symbol),
    ))
}

/// The function type an operator head declares, beside its symbol: `FN :{left :Operand, right
/// :Operand} -> Ret` for a binary `OP`, returning its declared result or else its operand since a
/// chain of it folds, and `FN :{operands :(LIST OF Operand)} -> Ret` for a `UNARY OP`, since its
/// body takes the whole run. The parameter names are the ones the shape builder binds in the
/// operator's body.
fn operator_function<'graph, XF: KnottedFamily<'graph>>(
    elaborator: &Elaborator<'_, '_, 'graph, '_, '_, XF>,
    unary: bool,
    symbol: &ExpressionPart<'graph>,
    operand: &ExpressionPart<'graph>,
    ret: Option<&ExpressionPart<'graph>>,
    groups: &Groups<'_>,
) -> Result<(KeywordSymbol, KType), Elaboration> {
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
        types.function_type(scratch, &[], &[(operands, types.list(operand))], ret)
    } else {
        let left = BinderSymbol::Value(IMPLICIT.left.symbol());
        let right = BinderSymbol::Value(IMPLICIT.right.symbol());
        types.function_type(scratch, &[], &[(left, operand), (right, operand)], ret)
    };
    Ok((symbol, function.handle))
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

/// The expression shape a registration puts in its bucket, built from `function`, the definition's
/// function type, over `head`: each keyword where the head writes it, and each slot at the type
/// `function` gives the parameter it names, over `function`'s return and quantified over its
/// canonical group.
///
/// The shape door renumbers that group by first occurrence in element order, so the shape is the
/// one `:(EXPR …)` spells for the same head, although `function` numbered it in its parameters'
/// sorted order.
fn registered_shape(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    function: KType,
    head: Head<'_, '_>,
) -> KType {
    let TypeNode::KFunction {
        quantifiers,
        params,
        ret,
        ..
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
    let mut elements = BumpVec::new_in(scratch);
    match head {
        Head::Written(run) => walk_head(run, (), |element| {
            elements.push(match element {
                HeadElement::Keyword(symbol) => DispatchTokenElement::Keyword(symbol),
                HeadElement::Slot(name, _) => {
                    slot(name.expect("`head_function` refused a `_` slot").symbol())
                }
            });
            Ok(())
        })
        .expect("`head_function` refused every other part"),
        Head::Binary(symbol) => elements.extend([
            slot(IMPLICIT.left.symbol().symbol()),
            DispatchTokenElement::Keyword(symbol),
            slot(IMPLICIT.right.symbol().symbol()),
        ]),
        Head::Unary(symbol) => elements.extend([
            DispatchTokenElement::Keyword(symbol),
            slot(IMPLICIT.operands.symbol().symbol()),
        ]),
    }
    types
        .shape_type(scratch, quantifiers, &elements, ret)
        .handle
}
