//! Fast-lane dispatch shapes — bare identifier, bare leaf type,
//! constructor call, sigiled type expression, literal pass-through.
//! All terminate (or single-producer-park) in one poll except
//! `ConstructorCall`, which can park on per-value-cell eager-subs and
//! resume via [`CtorState::resume`].

use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::builtins::newtype_def::newtype_construct;
use super::coerce_type_token_value;
use super::constructors;
use crate::machine::core::kfunction::BodyResult;
use crate::machine::core::source::Spanned;
use crate::machine::core::ScopeId;
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
    pub(in crate::machine::execute) track: Option<CtorTrack<'a>>,
}

/// Pending eager-subs for a parked `ConstructorCall`. `staged_values`
/// already holds the slots whose dispatch short-circuited at install
/// time (an arena-resident `&KObject`); `subs` carries `(slot_index,
/// sub_id)` for the remaining parked slots. The resume reads each
/// sub's terminal, fills the slot, and tail-calls
/// [`constructors::finish`].
pub(in crate::machine::execute) struct CtorTrack<'a> {
    pub(in crate::machine::execute) subs: Vec<(usize, NodeId)>,
    pub(in crate::machine::execute) staged_values: Vec<Option<&'a KObject<'a>>>,
    pub(in crate::machine::execute) kind: CtorKind<'a>,
}

/// Schema-keyed payload the resume needs to materialize the
/// constructed value once every slot is resolved.
pub(in crate::machine::execute) enum CtorKind<'a> {
    Struct {
        name: String,
        scope_id: ScopeId,
        fields: Rc<Vec<(String, KType<'a>)>>,
    },
    Tagged {
        schema: Rc<HashMap<String, KType<'a>>>,
        name: String,
        scope_id: ScopeId,
        tag: String,
    },
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
        Self { init, track: None }
    }

    pub(in crate::machine::execute) fn with_track(init: Initialized, track: CtorTrack<'a>) -> Self {
        Self { init, track: Some(track) }
    }

    /// Drain the parked subs into `staged_values` and tail-call
    /// `constructors::finish` once every slot is bound. Errors on a
    /// dep terminate the resume with that error.
    pub(in crate::machine::execute) fn resume(
        self,
        ctx: &mut DispatchCtx<'a, '_>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let CtorState { init, track } = self;
        let _ = init;
        let CtorTrack { subs, mut staged_values, kind } =
            track.expect("ConstructorCall resume only entered after a track is installed");
        for (slot_idx, sub_id) in &subs {
            match ctx.read_result(*sub_id) {
                Ok(v) => staged_values[*slot_idx] = Some(v),
                Err(e) => {
                    let err = e.clone_for_propagation();
                    ctx.clear_dep_edges(idx);
                    for (_, dep_id) in &subs {
                        ctx.free(dep_id.index());
                    }
                    return Ok(NodeStep::Done(NodeOutput::Err(err)));
                }
            }
        }
        ctx.clear_dep_edges(idx);
        for (_, dep_id) in &subs {
            ctx.free(dep_id.index());
        }
        let values: Vec<&'a KObject<'a>> = staged_values.into_iter().map(|o| o.unwrap()).collect();
        Ok(constructors::finish(scope, &kind, &values))
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
    // Fresh `types[name]` lookup at construction time. The schema payload rides the
    // identity, so a recursive type whose cycle-close pre-installed a payload-empty
    // identity reads the schema-bearing one that finalize's upsert replaced it with —
    // no value-side carrier involved.
    let identity = match scope.resolve_type_with_chain(&head_t.name, chain) {
        Some(kt) => kt,
        None => {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(
                head_t.name.clone(),
            ))));
        }
    };
    match identity {
        KType::UserType { kind: UserTypeKind::Struct { fields }, scope_id, name } => {
            constructors::dispatch_construct_struct(
                ctx,
                name.clone(),
                *scope_id,
                Rc::clone(fields),
                inner_parts,
                scope,
                idx,
            )
        }
        KType::UserType { kind: UserTypeKind::Tagged { schema }, scope_id, name } => {
            constructors::dispatch_construct_tagged(
                ctx,
                name.clone(),
                *scope_id,
                Rc::clone(schema),
                inner_parts,
                scope,
                idx,
            )
        }
        KType::UserType { kind: UserTypeKind::Newtype { .. }, .. } => {
            let body = newtype_construct(scope, ctx, identity, inner_parts);
            schedule_constructor_body(ctx, body, idx)
        }
        KType::UserType { kind: UserTypeKind::TypeConstructor { schema, .. }, scope_id, name } => {
            constructors::dispatch_construct_tagged(
                ctx,
                name.clone(),
                *scope_id,
                Rc::clone(schema),
                inner_parts,
                scope,
                idx,
            )
        }
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
