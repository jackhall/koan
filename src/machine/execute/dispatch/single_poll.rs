//! Fast-lane dispatch shapes — bare identifier, bare leaf type,
//! bare-`Type`-head call, sigiled type expression, literal pass-through.
//! Most terminate (or single-producer-park) in one poll. Two carry a resume:
//! `TypeCall` parks on per-value-cell eager-subs (or a still-finalizing head
//! binding) and resumes via [`CtorState::resume`]; `BareTypeLeaf` parks on a
//! still-finalizing referent and re-resolves via [`BareTypeState::resume`].

use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use super::{resolve_type_leaf_carrier, TypeLeafCarrier};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeName};
use crate::machine::model::{KType, RecursiveSet};
use crate::machine::{KError, KErrorKind, NodeId, Resolution};

use super::super::nodes::{DispatchCombineFinish, NodeOutput, NodeStep};
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::DispatchCx;
use super::outcome::{DispatchDep, DispatchOutcome};
use super::{DispatchState, Initialized};

pub(in crate::machine::execute) struct BareTypeState<'run> {
    /// Set when `bare_type_leaf` parked on a still-finalizing referent (a
    /// `RECURSIVE TYPES` member caught mid-seal). On resume the leaf re-resolves
    /// against the now-sealed binding through the same memoized bridge.
    pub(in crate::machine::execute) park: Option<BareTypeParkTrack>,
    _ph: PhantomData<&'run ()>,
}

/// Parked resolution state for a `BareTypeLeaf` whose referent was still finalizing.
/// Carries the leaf `TypeName` so the resume re-runs the resolve once the single producer
/// is sealed; the producer's terminal is not the type carrier, so the resume re-resolves
/// (hitting the sealed memo) rather than lifting the producer's value.
pub(in crate::machine::execute) struct BareTypeParkTrack {
    pub(in crate::machine::execute) leaf: TypeName,
    pub(in crate::machine::execute) producer: NodeId,
}

/// Parked `TypeCall` state. Ctor value subs park as a [`NodeWork::DispatchCombine`]
/// (`constructors::launch`), so the only thing a `CtorState` carries is a still-finalizing
/// *head* binding: `type_call` parked on a `LET <Type-class> = …` placeholder (e.g. a forward
/// functor). On resume the whole `type_call` re-runs against the now-finalized binding — the
/// head may resolve type-side (a functor or type alias), so the keyworded resolve path is the
/// wrong continuation.
pub(in crate::machine::execute) struct CtorState<'run> {
    pub(in crate::machine::execute) init: Initialized,
    pub(in crate::machine::execute) head_placeholder: TypeCallHeadPlaceholder<'run>,
}

/// Parked head-resolution state for a `TypeCall` whose head name was a
/// still-finalizing placeholder. Carries the original call expression so the
/// resume re-runs the fast lane once the producer is bound.
pub(in crate::machine::execute) struct TypeCallHeadPlaceholder<'run> {
    pub(in crate::machine::execute) expr: KExpression<'run>,
    pub(in crate::machine::execute) producer: NodeId,
}

/// Schema-keyed payload the resume needs to materialize the constructed value once every
/// slot is resolved. `(set, index)` is the sealed-member identity stamped onto the produced
/// `KObject`; `schema` is the projected (sibling-`SetLocal`-resolved) schema used for
/// per-value type-checking.
pub(in crate::machine::execute) enum CtorKind<'run> {
    /// Newtype construction (record-repr or scalar) from a single positional value. One value
    /// cell carrying the whole value expression; the finish type-checks it against the
    /// member's `repr`, peels any `Wrapped` layer, and tags it with `identity`.
    Newtype { identity: &'run KType<'run> },
    /// Record-repr newtype construction from a named record-literal body (`Point {x = 1, y =
    /// 2}`). One value cell per field, so a literal field stages in place (synchronous bind,
    /// matching the retired struct path) instead of deferring the whole record literal; the
    /// finish builds the `KObject::Record` and wraps it with `identity`.
    RecordNewtype {
        identity: &'run KType<'run>,
        field_names: Vec<String>,
    },
    Tagged {
        schema: Rc<HashMap<String, KType<'run>>>,
        set: Rc<RecursiveSet<'run>>,
        index: usize,
        tag: String,
    },
}

impl<'run> BareTypeState<'run> {
    pub(in crate::machine::execute) fn with_park(park: BareTypeParkTrack) -> Self {
        Self {
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
        ctx: &DispatchCx<'run, '_>,
    ) -> DispatchOutcome<'run> {
        let BareTypeState { park, .. } = self;
        let BareTypeParkTrack { leaf, producer } =
            park.expect("BareTypeLeaf resume only entered after a park track is installed");
        // The producer's terminal is not the type carrier; the resume re-resolves through
        // the now-sealed memo rather than reading the producer's value. The dep edges are
        // cleared by the router before this decide runs.
        let _ = producer;
        bare_type_leaf(ctx, &leaf)
    }
}

impl<'run> CtorState<'run> {
    pub(in crate::machine::execute) fn with_head_placeholder(
        init: Initialized,
        head_placeholder: TypeCallHeadPlaceholder<'run>,
    ) -> Self {
        Self {
            init,
            head_placeholder,
        }
    }

    /// Re-run `type_call` against the now-finalized head binding. Ctor value subs resolve
    /// through their own `DispatchCombine`, so a `CtorState` only ever parks on a head
    /// placeholder.
    pub(in crate::machine::execute) fn resume(
        self,
        ctx: &DispatchCx<'run, '_>,
    ) -> DispatchOutcome<'run> {
        let CtorState {
            init,
            head_placeholder: TypeCallHeadPlaceholder { expr, producer },
        } = self;
        let _ = init;
        let _ = producer;
        // The dep edges are cleared by the router before this decide runs.
        type_call(ctx, expr)
    }
}

/// Surfaces `UnboundName` directly when the name has no binding and
/// no visible placeholder — no dispatch retry, no overload search.
pub(super) fn bare_identifier<'run>(
    ctx: &DispatchCx<'run, '_>,
    name: String,
) -> DispatchOutcome<'run> {
    match ctx
        .current_scope()
        .resolve_with_chain(&name, ctx.chain_deref())
    {
        Resolution::Value(obj) => DispatchOutcome::Terminal(NodeOutput::value(obj)),
        Resolution::Placeholder(producer) => DispatchOutcome::ParkLift { producer },
        Resolution::UnboundName => {
            DispatchOutcome::Terminal(NodeOutput::Err(KError::new(KErrorKind::UnboundName(name))))
        }
    }
}

pub(super) fn bare_type_leaf<'run>(
    ctx: &DispatchCx<'run, '_>,
    t: &TypeName,
) -> DispatchOutcome<'run> {
    match resolve_type_leaf_carrier(ctx.current_scope(), t, ctx.active_chain()) {
        TypeLeafCarrier::Resolved(kt) => DispatchOutcome::Terminal(NodeOutput::ktype(kt)),
        TypeLeafCarrier::Unbound(n) => {
            DispatchOutcome::Terminal(NodeOutput::Err(KError::new(KErrorKind::UnboundName(n))))
        }
        // A still-finalizing referent. A visible type alias has already resolved its RHS
        // through the bridge, so a bare leaf parks on exactly one producer; park on it and
        // re-resolve once it seals. A producer already terminal-with-error short-circuits.
        TypeLeafCarrier::Park(producers) => {
            let producer = match producers.first() {
                Some(p) => *p,
                None => {
                    return DispatchOutcome::Terminal(NodeOutput::Err(KError::new(
                        KErrorKind::UnboundName(t.render()),
                    )));
                }
            };
            if ctx.is_result_ready(producer) {
                if let Err(e) = ctx.read_result(producer) {
                    return DispatchOutcome::Terminal(NodeOutput::Err(e.clone_for_propagation()));
                }
                // Ready-and-bound: the referent finalized between resolve and this check, so
                // re-resolve directly — the memoized bridge now admits.
                return bare_type_leaf(ctx, t);
            }
            let track = BareTypeParkTrack {
                leaf: t.clone(),
                producer,
            };
            DispatchOutcome::ParkSelf {
                producers: vec![producer],
                state: DispatchState::BareTypeLeaf(BareTypeState::with_park(track)),
            }
        }
    }
}

pub(super) fn sigiled_type_expr<'run>(expr: KExpression<'run>) -> DispatchOutcome<'run> {
    let inner = match expr.parts.into_iter().next() {
        Some(Spanned {
            value: ExpressionPart::SigiledTypeExpr(boxed),
            ..
        }) => *boxed,
        _ => unreachable!("SigiledTypeExpr shape implies single SigiledTypeExpr part"),
    };
    DispatchOutcome::BecomeDispatch(inner)
}

/// `:{x :Number, y :Str}` — a single-part record-type sigil. Folds the field list straight
/// to `KObject::KTypeValue(KType::Record(_))` via the shared field-list elaborator, deferring
/// through a Combine when a field forward-references or sub-dispatches. No type-constructor
/// builtin is involved — the record type is structural.
pub(super) fn record_type<'run>(
    ctx: &DispatchCx<'run, '_>,
    expr: KExpression<'run>,
) -> DispatchOutcome<'run> {
    let fields = match expr.parts.into_iter().next() {
        Some(Spanned {
            value: ExpressionPart::RecordType(boxed),
            ..
        }) => *boxed,
        _ => unreachable!("RecordType shape implies a single RecordType part"),
    };
    let chain = ctx.current_lexical_chain();
    DispatchOutcome::ElaborateRecordType { fields, chain }
}

/// `(99)`, `("x")`, `([1 2 3])`, `((inner))` etc. — single-part
/// literal-shaped expressions. Skips the bucket lookup + builtin call
/// the Keyworded path would otherwise route through.
pub(super) fn literal_pass_through<'run>(
    ctx: &DispatchCx<'run, '_>,
    expr: KExpression<'run>,
) -> DispatchOutcome<'run> {
    let only = expr
        .parts
        .into_iter()
        .next()
        .expect("LiteralPassThrough shape implies one part");
    match only.value {
        ExpressionPart::Literal(_) => {
            let allocated = ctx.current_scope().arena.alloc_object(only.value.resolve());
            DispatchOutcome::Terminal(NodeOutput::value(allocated))
        }
        ExpressionPart::Future(c) => DispatchOutcome::Terminal(NodeOutput::Value(c)),
        ExpressionPart::Expression(boxed) => DispatchOutcome::BecomeDispatch(*boxed),
        ExpressionPart::ListLiteral(items) => park_on_literal(DispatchDep::ListLit(items)),
        ExpressionPart::DictLiteral(pairs) => park_on_literal(DispatchDep::DictLit(pairs)),
        ExpressionPart::RecordLiteral(fields) => park_on_literal(DispatchDep::RecordLit(fields)),
        _ => unreachable!("LiteralPassThrough classifier only routes Literal/Future/Expression/ListLiteral/DictLiteral/RecordLiteral"),
    }
}

/// Park the slot on a single literal-producer dep as a [`DispatchOutcome::Combine`] whose finish
/// lifts the producer's resolved value straight through. The harness submits the literal and owns
/// it; a dep error short-circuits frameless before the finish runs.
fn park_on_literal<'run>(dep: DispatchDep<'run>) -> DispatchOutcome<'run> {
    let finish: DispatchCombineFinish<'run> =
        Box::new(|_ctx, results, _idx| NodeStep::Done(NodeOutput::Value(results[0])));
    DispatchOutcome::Combine {
        deps: vec![dep],
        dep_error_frame: None,
        finish,
        free: Vec::new(),
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
pub(super) fn type_call<'run>(
    ctx: &DispatchCx<'run, '_>,
    expr: KExpression<'run>,
) -> DispatchOutcome<'run> {
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
    if let Resolution::Placeholder(producer) = ctx
        .current_scope()
        .resolve_with_chain(head_t.as_str(), chain)
    {
        if !ctx.is_result_ready(producer) {
            let init = Initialized {
                pre_subs: Vec::new(),
            };
            let head_placeholder = TypeCallHeadPlaceholder { expr, producer };
            return DispatchOutcome::ParkSelf {
                producers: vec![producer],
                state: DispatchState::TypeCall(Box::new(CtorState::with_head_placeholder(
                    init,
                    head_placeholder,
                ))),
            };
        }
    }
    // Fresh `types[name]` lookup at construction time. A sealed nominal type's identity is
    // a `SetRef` whose member carries the schema (filled at the member's finalize) — no
    // value-side carrier involved. A bound functor lives here too, carrying its callable
    // body on `KType::KFunctor { body: Some(f) }`.
    let identity = match ctx
        .current_scope()
        .resolve_type_with_chain(head_t.as_str(), chain)
    {
        Some(kt) => kt,
        None => {
            return DispatchOutcome::Terminal(NodeOutput::Err(KError::new(
                KErrorKind::UnboundName(head_t.render()),
            )));
        }
    };
    match identity {
        // A bound functor's result is a module — the `Function` arm calls it.
        KType::KFunctor { body: Some(f), .. } => {
            apply_callable(ctx, ResolvedCallable::Function(f), &expr)
        }
        // A bare `:(FUNCTOR …)` type annotation has no callable to invoke.
        KType::KFunctor { body: None, .. } => {
            DispatchOutcome::Terminal(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "constructible Type or bound functor".to_string(),
                got: identity.name(),
            })))
        }
        _ => apply_callable(ctx, ResolvedCallable::Constructor(identity), &expr),
    }
}

