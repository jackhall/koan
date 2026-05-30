//! Single-poll dispatch shapes — bare identifier, bare leaf type,
//! constructor call, sigiled type expression. All four terminalize
//! (or single-producer-park) in one poll and never re-enter.

use std::marker::PhantomData;

use crate::builtins::newtype_def::newtype_construct;
use crate::builtins::{dispatch_constructor, struct_value, tagged_union};
use super::coerce_type_token_value;
use crate::machine::core::kfunction::BodyResult;
use crate::machine::core::source::Spanned;
use crate::machine::model::Parseable;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, NodeId, Resolution, Scope};

use super::super::nodes::{LiftState, NodeOutput, NodeStep, NodeWork};
use super::{DispatchCtx, extract_named_call_inner, keyworded::KeywordedState, Initialized};

pub(in crate::machine::execute) struct BareIdState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct BareTypeState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct CtorState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct SigilState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct LitState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: PhantomData<&'a ()>,
}

impl<'a> BareIdState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: PhantomData }
    }
}

impl<'a> BareTypeState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: PhantomData }
    }
}

impl<'a> CtorState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: PhantomData }
    }
}

impl<'a> SigilState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: PhantomData }
    }
}

impl<'a> LitState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, _ph: PhantomData }
    }
}

/// Surfaces `UnboundName` directly when the name has no binding and
/// no visible placeholder — no dispatch retry, no overload search.
pub(super) fn bare_identifier<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    name: String,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    match scope.resolve_with_chain(&name, ctx.chain_deref()) {
        Resolution::Value(obj) => NodeStep::Done(NodeOutput::Value(obj)),
        Resolution::Placeholder(producer) => {
            // Notify edge, not Owned: producer is a sibling slot, we
            // only park for the wake.
            ctx.add_park_edge(producer, NodeId(idx));
            NodeStep::Replace {
                work: NodeWork::Lift(LiftState::Pending(producer)),
                frame: None,
                function: None,
                block_entry: None,
                body_index: 0,
            }
        }
        Resolution::UnboundName => {
            NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(name))))
        }
    }
}

pub(super) fn bare_type_leaf<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    t: &TypeExpr,
    scope: &'a Scope<'a>,
) -> NodeStep<'a> {
    let chain = ctx.chain_deref();
    match coerce_type_token_value(scope, t, chain) {
        Ok(obj) => NodeStep::Done(NodeOutput::Value(obj)),
        Err(KError { kind: KErrorKind::UnboundName(n), .. }) => {
            NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(n))))
        }
        Err(e) => NodeStep::Done(NodeOutput::Err(e)),
    }
}

pub(super) fn sigiled_type_expr<'a>(expr: KExpression<'a>) -> NodeStep<'a> {
    let inner = match expr.parts.into_iter().next() {
        Some(Spanned { value: ExpressionPart::SigiledTypeExpr(boxed), .. }) => *boxed,
        _ => unreachable!("SigiledTypeExpr shape implies single SigiledTypeExpr part"),
    };
    NodeStep::Replace {
        work: NodeWork::dispatch(inner),
        frame: None,
        function: None,
        block_entry: None,
        body_index: 0,
    }
}

/// `(99)`, `("x")`, `([1 2 3])`, `((inner))` etc. — single-part
/// literal-shaped expressions. Skips the bucket lookup + builtin call
/// the Keyworded path would otherwise route through.
pub(super) fn literal_pass_through<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    expr: KExpression<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let only = expr.parts.into_iter().next().expect("LiteralPassThrough shape implies one part");
    match only.value {
        ExpressionPart::Literal(_) => {
            let allocated = scope.arena.alloc(only.value.resolve());
            NodeStep::Done(NodeOutput::Value(allocated))
        }
        ExpressionPart::Future(obj) => NodeStep::Done(NodeOutput::Value(obj)),
        ExpressionPart::Expression(boxed) => NodeStep::Replace {
            work: NodeWork::dispatch(*boxed),
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        },
        ExpressionPart::ListLiteral(items) => {
            let producer = ctx.schedule_list_literal(items, scope);
            park_on_literal_producer(ctx, producer, idx)
        }
        ExpressionPart::DictLiteral(pairs) => {
            let producer = ctx.schedule_dict_literal(pairs, scope);
            park_on_literal_producer(ctx, producer, idx)
        }
        _ => unreachable!("LiteralPassThrough classifier only routes Literal/Future/Expression/ListLiteral/DictLiteral"),
    }
}

/// Either lift the producer's already-ready value, or park on it via a
/// `Lift(Pending)`. Owned-edge install mirrors `install_eager_subs`.
fn park_on_literal_producer<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    producer: NodeId,
    idx: usize,
) -> NodeStep<'a> {
    if ctx.is_result_ready(producer) {
        let outcome = match ctx.read_result(producer) {
            Ok(v) => NodeOutput::Value(v),
            Err(e) => NodeOutput::Err(e.clone_for_propagation()),
        };
        ctx.free(producer.index());
        return NodeStep::Done(outcome);
    }
    ctx.add_owned_edge(producer, NodeId(idx));
    NodeStep::Replace {
        work: NodeWork::Lift(LiftState::Pending(producer)),
        frame: None,
        function: None,
        block_entry: None,
        body_index: 0,
    }
}

/// A forward-reference `Placeholder` on the head name parks via
/// `install_overload_park` (single-producer is fine — the installer
/// dedupes / cycle-filters internally) so the resume rebuilds via
/// `KeywordedState::initial`.
pub(super) fn constructor_call<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    expr: KExpression<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let head_t = match &expr.parts[0].value {
        ExpressionPart::Type(t) => t.clone(),
        _ => unreachable!("ConstructorCall shape implies leaf Type head"),
    };
    let inner_parts = match extract_named_call_inner(&expr) {
        Ok(parts) => parts,
        Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
    };
    let chain = ctx.chain_deref();
    match scope.resolve_with_chain(&head_t.name, chain) {
        Resolution::Placeholder(producer) => {
            return KeywordedState::install_overload_park(
                ctx,
                vec![producer],
                expr,
                Vec::new(),
                idx,
            );
        }
        Resolution::Value(_) | Resolution::UnboundName => {}
    }
    let identity = match scope.resolve_type_with_chain(&head_t.name, chain) {
        Some(kt) => kt,
        None => {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(
                head_t.name.clone(),
            ))));
        }
    };
    match identity {
        KType::UserType { kind: UserTypeKind::Struct, .. }
        | KType::UserType { kind: UserTypeKind::Tagged, .. } => {
            let carrier = match coerce_type_token_value(scope, &head_t, chain) {
                Ok(obj) => obj,
                Err(KError { kind: KErrorKind::UnboundName(n), .. }) => {
                    return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(
                        n,
                    ))));
                }
                Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
            };
            let body = match carrier {
                KObject::StructType { .. } => struct_value::apply(carrier, inner_parts),
                KObject::TaggedUnionType { .. } => tagged_union::apply(carrier, inner_parts),
                other => {
                    debug_assert!(
                        false,
                        "STRUCT/UNION `{}` registered its type identity but no \
                         matching value-side schema carrier (got `{}`)",
                        head_t.name,
                        other.summarize(),
                    );
                    return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                        arg: "verb".to_string(),
                        expected: "constructible Type".to_string(),
                        got: identity.name(),
                    })));
                }
            };
            schedule_constructor_body(ctx, body, idx)
        }
        KType::UserType { kind: UserTypeKind::Newtype { .. }, .. } => {
            let body = newtype_construct(scope, ctx, identity, inner_parts);
            schedule_constructor_body(ctx, body, idx)
        }
        KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. } => match scope
            .lookup_with_chain(&head_t.name, chain)
            .and_then(|c| dispatch_constructor(c, inner_parts))
        {
            Some(body) => schedule_constructor_body(ctx, body, idx),
            None => NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "constructible Type".to_string(),
                got: identity.name(),
            }))),
        },
        _ => NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
            arg: "verb".to_string(),
            expected: "constructible Type".to_string(),
            got: identity.name(),
        }))),
    }
}

/// Decode a constructor `BodyResult` into a `NodeStep`.
pub(super) fn schedule_constructor_body<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    body: BodyResult<'a>,
    idx: usize,
) -> NodeStep<'a> {
    match body {
        BodyResult::Tail { expr, frame, function, block_entry, body_index } => NodeStep::Replace {
            work: NodeWork::dispatch(expr),
            frame,
            function,
            block_entry,
            body_index,
        },
        BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
        BodyResult::DeferTo(combine_id) => ctx.defer_to_lift(idx, combine_id),
        BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
    }
}
