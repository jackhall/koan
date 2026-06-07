//! Fast-lane dispatch shapes — bare identifier, bare leaf type,
//! bare-`Type`-head call, sigiled type expression, literal pass-through.
//! Most terminate (or single-producer-park) in one poll. Two carry a resume:
//! `TypeCall` parks on per-value-cell eager-subs (or a still-finalizing head
//! binding) and resumes via [`CtorState::resume`]; `BareTypeLeaf` parks on a
//! still-finalizing referent and re-resolves via [`BareTypeState::resume`].

use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use super::constructors;
use super::{resolve_type_leaf_carrier, TypeLeafCarrier};
use crate::machine::core::kfunction::BodyResult;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeName};
use crate::machine::model::{KObject, KType, RecursiveSet};
use crate::machine::{KError, KErrorKind, NodeId, Resolution, SchedulerHandle, Scope};

use super::super::nodes::{LiftState, NodeOutput, NodeStep, NodeWork};
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::{DispatchCtx, DispatchState, Initialized};

pub(in crate::machine::execute) struct BareIdState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    _ph: PhantomData<&'a ()>,
}

pub(in crate::machine::execute) struct BareTypeState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    /// Set when `bare_type_leaf` parked on a still-finalizing referent (a
    /// `RECURSIVE TYPES` member caught mid-seal). On resume the leaf re-resolves
    /// against the now-sealed binding through the same memoized bridge.
    pub(in crate::machine::execute) park: Option<BareTypeParkTrack>,
    _ph: PhantomData<&'a ()>,
}

/// Parked resolution state for a `BareTypeLeaf` whose referent was still finalizing.
/// Carries the leaf `TypeName` so the resume re-runs the resolve once the single producer
/// is sealed; the producer's terminal is not the type carrier, so the resume re-resolves
/// (hitting the sealed memo) rather than lifting the producer's value.
pub(in crate::machine::execute) struct BareTypeParkTrack {
    pub(in crate::machine::execute) leaf: TypeName,
    pub(in crate::machine::execute) producer: NodeId,
}

pub(in crate::machine::execute) struct CtorState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    pub(in crate::machine::execute) track: Option<CtorTrack<'a>>,
    /// Set when `type_call` parked on a still-finalizing head binding (a
    /// `LET <Type-class> = …` placeholder, e.g. a forward functor). On resume
    /// the whole `type_call` re-runs against the now-finalized binding — the
    /// head may resolve type-side (a functor or type alias), so the keyworded
    /// resolve path is the wrong continuation. Mutually exclusive with `track`.
    pub(in crate::machine::execute) head_placeholder: Option<TypeCallHeadPlaceholder<'a>>,
}

/// Parked head-resolution state for a `TypeCall` whose head name was a
/// still-finalizing placeholder. Carries the original call expression so the
/// resume re-runs the fast lane once the producer is bound.
pub(in crate::machine::execute) struct TypeCallHeadPlaceholder<'a> {
    pub(in crate::machine::execute) expr: KExpression<'a>,
    pub(in crate::machine::execute) producer: NodeId,
}

/// Pending eager-subs for a parked `TypeCall`. `staged_values`
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

/// Schema-keyed payload the resume needs to materialize the constructed value once every
/// slot is resolved. `(set, index)` is the sealed-member identity stamped onto the produced
/// `KObject`; `schema` is the projected (sibling-`SetLocal`-resolved) schema used for
/// per-value type-checking.
pub(in crate::machine::execute) enum CtorKind<'a> {
    /// Newtype construction (record-repr or scalar) from a single positional value. One value
    /// cell carrying the whole value expression; the finish type-checks it against the
    /// member's `repr`, peels any `Wrapped` layer, and tags it with `identity`.
    Newtype { identity: &'a KType<'a> },
    /// Record-repr newtype construction from a named record-literal body (`Point {x = 1, y =
    /// 2}`). One value cell per field, so a literal field stages in place (synchronous bind,
    /// matching the retired struct path) instead of deferring the whole record literal; the
    /// finish builds the `KObject::Record` and wraps it with `identity`.
    RecordNewtype {
        identity: &'a KType<'a>,
        field_names: Vec<String>,
    },
    Tagged {
        schema: Rc<HashMap<String, KType<'a>>>,
        set: Rc<RecursiveSet<'a>>,
        index: usize,
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
        Self {
            init,
            _ph: PhantomData,
        }
    }
}

impl<'a> BareTypeState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self {
            init,
            park: None,
            _ph: PhantomData,
        }
    }

    pub(in crate::machine::execute) fn with_park(
        init: Initialized,
        park: BareTypeParkTrack,
    ) -> Self {
        Self {
            init,
            park: Some(park),
            _ph: PhantomData,
        }
    }

    /// Re-run `bare_type_leaf` against the now-sealed referent. The producer's terminal is
    /// not the type carrier (a finalize-combine returns its own value), so this re-resolves
    /// through the memoized bridge — a hit on the sealed `type_expr_memo` — rather than
    /// lifting the producer's value.
    pub(in crate::machine::execute) fn resume(
        self,
        ctx: &mut DispatchCtx<'a, '_>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let BareTypeState { park, .. } = self;
        let BareTypeParkTrack { leaf, producer } =
            park.expect("BareTypeLeaf resume only entered after a park track is installed");
        // The producer's terminal is not the type carrier; the resume re-resolves through
        // the now-sealed memo rather than reading the producer's value.
        let _ = producer;
        ctx.clear_dep_edges(idx);
        Ok(bare_type_leaf(ctx, &leaf, scope, idx))
    }
}

impl<'a> CtorState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self {
            init,
            track: None,
            head_placeholder: None,
        }
    }

    pub(in crate::machine::execute) fn with_track(init: Initialized, track: CtorTrack<'a>) -> Self {
        Self {
            init,
            track: Some(track),
            head_placeholder: None,
        }
    }

    pub(in crate::machine::execute) fn with_head_placeholder(
        init: Initialized,
        head_placeholder: TypeCallHeadPlaceholder<'a>,
    ) -> Self {
        Self {
            init,
            track: None,
            head_placeholder: Some(head_placeholder),
        }
    }

    /// Drain the parked subs into `staged_values` and tail-call
    /// `constructors::finish` once every slot is bound. Errors on a
    /// dep terminate the resume with that error. A `head_placeholder` resume
    /// instead re-runs `type_call` against the now-finalized head binding.
    pub(in crate::machine::execute) fn resume(
        self,
        ctx: &mut DispatchCtx<'a, '_>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let CtorState {
            init,
            track,
            head_placeholder,
        } = self;
        let _ = init;
        if let Some(TypeCallHeadPlaceholder { expr, producer }) = head_placeholder {
            debug_assert!(
                track.is_none(),
                "head_placeholder and eager-subs track are mutually exclusive",
            );
            let _ = producer;
            ctx.clear_dep_edges(idx);
            return Ok(type_call(ctx, expr, scope, idx));
        }
        let CtorTrack {
            subs,
            mut staged_values,
            kind,
        } = track.expect("TypeCall resume only entered after a track is installed");
        for (slot_idx, sub_id) in &subs {
            match ctx.read_result(*sub_id) {
                Ok(v) => staged_values[*slot_idx] = Some(v.object()),
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
        Self {
            init,
            _ph: PhantomData,
        }
    }
}

impl<'a> LitState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self {
            init,
            _ph: PhantomData,
        }
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
        Resolution::Value(obj) => NodeStep::Done(NodeOutput::value(obj)),
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
    t: &TypeName,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    match resolve_type_leaf_carrier(scope, t, ctx.active_chain()) {
        TypeLeafCarrier::Resolved(obj) => NodeStep::Done(NodeOutput::value(obj)),
        TypeLeafCarrier::Unbound(n) => {
            NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(n))))
        }
        // A still-finalizing referent. A visible type alias has already resolved its RHS
        // through the bridge, so a bare leaf parks on exactly one producer; park on it and
        // re-resolve once it seals. A producer already terminal-with-error short-circuits.
        TypeLeafCarrier::Park(producers) => {
            let producer = match producers.first() {
                Some(p) => *p,
                None => {
                    return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(
                        t.render(),
                    ))));
                }
            };
            if ctx.is_result_ready(producer) {
                if let Err(e) = ctx.read_result(producer) {
                    return NodeStep::Done(NodeOutput::Err(e.clone_for_propagation()));
                }
                // Ready-and-bound: the referent finalized between resolve and this check, so
                // re-resolve directly — the memoized bridge now admits.
                return bare_type_leaf(ctx, t, scope, idx);
            }
            ctx.add_park_edge(producer, NodeId(idx));
            let init = Initialized {
                pre_subs: Vec::new(),
            };
            let track = BareTypeParkTrack {
                leaf: t.clone(),
                producer,
            };
            ctx.replace_with_parked_dispatch(DispatchState::BareTypeLeaf(BareTypeState::with_park(
                init, track,
            )))
        }
    }
}

pub(super) fn sigiled_type_expr<'a>(expr: KExpression<'a>) -> NodeStep<'a> {
    let inner = match expr.parts.into_iter().next() {
        Some(Spanned {
            value: ExpressionPart::SigiledTypeExpr(boxed),
            ..
        }) => *boxed,
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

/// `:{x :Number, y :Str}` — a single-part record-type sigil. Folds the field list straight
/// to `KObject::KTypeValue(KType::Record(_))` via the shared field-list elaborator, deferring
/// through a Combine when a field forward-references or sub-dispatches. No type-constructor
/// builtin is involved — the record type is structural.
pub(super) fn record_type<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    expr: KExpression<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let fields = match expr.parts.into_iter().next() {
        Some(Spanned {
            value: ExpressionPart::RecordType(boxed),
            ..
        }) => *boxed,
        _ => unreachable!("RecordType shape implies a single RecordType part"),
    };
    let chain = ctx.current_lexical_chain();
    let body = super::field_list::elaborate_record_value(scope, ctx, fields, chain);
    schedule_constructor_body(ctx, body, idx)
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
    let only = expr
        .parts
        .into_iter()
        .next()
        .expect("LiteralPassThrough shape implies one part");
    match only.value {
        ExpressionPart::Literal(_) => {
            let allocated = scope.arena.alloc_object(only.value.resolve());
            NodeStep::Done(NodeOutput::value(allocated))
        }
        ExpressionPart::Future(c) => NodeStep::Done(NodeOutput::Value(c)),
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
        ExpressionPart::RecordLiteral(fields) => {
            let producer = ctx.schedule_record_literal(fields, scope);
            park_on_literal_producer(ctx, producer, idx)
        }
        _ => unreachable!("LiteralPassThrough classifier only routes Literal/Future/Expression/ListLiteral/DictLiteral/RecordLiteral"),
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

/// Synchronous resolve-then-branch for a bare-`Type`-head call. One resolution,
/// branched outcomes routed through the shared apply-a-callable tail:
///
/// 1. `resolve_with_chain` first, only to park: a `Placeholder` (a still-finalizing
///    binding, including a recursive/forward functor) parks the whole dispatch via
///    `install_overload_park` until the producer finalizes. Bound *values* are not
///    branched here — every Type-class carrier (a type alias or a bound functor)
///    lives in the type table, read in step 2.
/// 2. `resolve_type_with_chain` (a pure `types[name]` read, no park) classifies the
///    identity:
///    - a constructible `SetRef` identity (a sealed nominal type) → the `Constructor` arm;
///    - a `KType::KFunctor { body: Some(f) }` (a bound functor) → the `Function`
///      arm — its result is a module;
///    - a `KType::KFunctor { body: None }` (a bare `:(FUNCTOR …)` annotation) is not
///      invocable → `TypeMismatch`;
///    - any other identity → `TypeMismatch`.
///
/// A name with no producer and no binding is `UnboundName` (genuine absence only —
/// pending names are already parked in step 1).
pub(super) fn type_call<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    expr: KExpression<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> NodeStep<'a> {
    let head_t = match &expr.parts[0].value {
        ExpressionPart::Type(t) => t.clone(),
        _ => unreachable!("TypeCall shape implies leaf Type head"),
    };
    let chain = ctx.chain_deref();
    // A still-finalizing head binding (a `LET <Type-class> = …` placeholder — e.g. a
    // forward functor) whose producer is not yet terminal: park on the producer and
    // re-run `type_call` on resume. Once the binding finalizes, the value-side
    // placeholder is cleared and the head resolves through the type table (a functor
    // or type alias), so a keyworded resume would wrongly fail. A producer that is
    // already terminal falls through — its placeholder is on its way out, so the
    // type-table read below is authoritative.
    if let Resolution::Placeholder(producer) = scope.resolve_with_chain(head_t.as_str(), chain) {
        if !ctx.is_result_ready(producer) {
            ctx.add_park_edge(producer, NodeId(idx));
            let init = Initialized {
                pre_subs: Vec::new(),
            };
            let head_placeholder = TypeCallHeadPlaceholder { expr, producer };
            return ctx.replace_with_parked_dispatch(DispatchState::TypeCall(Box::new(
                CtorState::with_head_placeholder(init, head_placeholder),
            )));
        }
    }
    // Fresh `types[name]` lookup at construction time. A sealed nominal type's identity is
    // a `SetRef` whose member carries the schema (filled at the member's finalize) — no
    // value-side carrier involved. A bound functor lives here too, carrying its callable
    // body on `KType::KFunctor { body: Some(f) }`.
    let identity = match scope.resolve_type_with_chain(head_t.as_str(), chain) {
        Some(kt) => kt,
        None => {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(
                head_t.render(),
            ))));
        }
    };
    match identity {
        // A bound functor's result is a module — the `Function` arm calls it.
        KType::KFunctor { body: Some(f), .. } => {
            apply_callable(ctx, ResolvedCallable::Function(f), &expr, scope, idx)
        }
        // A bare `:(FUNCTOR …)` type annotation has no callable to invoke.
        KType::KFunctor { body: None, .. } => {
            NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "constructible Type or bound functor".to_string(),
                got: identity.name(),
            })))
        }
        _ => apply_callable(
            ctx,
            ResolvedCallable::Constructor(identity),
            &expr,
            scope,
            idx,
        ),
    }
}

/// Decode a constructor `BodyResult` into a `NodeStep`.
pub(super) fn schedule_constructor_body<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    body: BodyResult<'a>,
    idx: usize,
) -> NodeStep<'a> {
    match body {
        BodyResult::Tail {
            expr,
            frame,
            function,
            block_entry,
            body_index,
        } => NodeStep::Replace {
            work: NodeWork::dispatch(expr),
            frame,
            function,
            block_entry,
            body_index,
        },
        BodyResult::Value(c) => NodeStep::Done(NodeOutput::Value(c)),
        BodyResult::DeferTo(combine_id) => ctx.defer_to_lift(idx, combine_id),
        BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
    }
}
