//! Fast-lane dispatch shapes — bare identifier, bare leaf type,
//! bare-`Type`-head call, sigiled type expression, literal pass-through.
//! Most terminate (or single-producer-park) in one poll. Two carry a resume:
//! `TypeCall` parks on per-value-cell eager-subs (its value subs as a
//! `AwaitDeps`) or on a still-finalizing head binding, re-running
//! [`type_call`] on wake; `BareTypeLeaf` parks on a still-finalizing referent
//! and re-resolves [`bare_type_leaf`]. Both park through a [`park_resume`] closure.

use std::collections::HashMap;
use std::rc::Rc;

use super::{resolve_type_leaf_carrier, TypeLeafCarrier};
use crate::machine::core::{KoanRegion, Scope};
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, KType, Parseable, RecursiveSet};
use crate::machine::{FrameSet, KError, KErrorKind, Resolution, ValueCarrierResolution};
use crate::source::Spanned;

use super::super::DepFinish;
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::SchedulerView;
use super::{become_dispatch, forward_to_producer, park_on_deps, park_resume, DepRequest, Outcome};

/// Schema-keyed payload the resume needs to materialize the constructed value once every
/// slot is resolved. `(set, index)` is the sealed-member identity stamped onto the produced
/// `KObject`; `schema` is the projected (sibling-`SetLocal`-resolved) schema used for
/// per-value type-checking.
pub(in crate::machine::execute) enum CtorKind<'step> {
    /// NewType construction (record-repr or scalar) from a single positional value. One value
    /// cell carrying the whole value expression; the finish type-checks it against the
    /// member's `repr`, peels any `Wrapped` layer, and tags it with `identity`.
    NewType { identity: &'step KType<'step> },
    /// Record-repr newtype construction from a named record-literal body (`Point {x = 1, y =
    /// 2}`). One value cell per field, so a literal field stages in place (synchronous bind,
    /// matching the retired struct path) instead of deferring the whole record literal; the
    /// finish builds the `KObject::Record` and wraps it with `identity`.
    RecordNewType {
        identity: &'step KType<'step>,
        field_names: Vec<String>,
    },
    Tagged {
        schema: Rc<HashMap<String, KType<'step>>>,
        set: Rc<RecursiveSet<'step>>,
        index: usize,
        tag: String,
    },
}

/// Surfaces `UnboundName` directly when the name has no binding and
/// no visible placeholder — no dispatch retry, no overload search.
pub(super) fn bare_identifier<'step, 'b>(
    ctx: &SchedulerView<'step, '_>,
    s: &'b Scope<'b>,
    name: String,
) -> Outcome<'step> {
    match s.resolve_value_carrier(&name, ctx.chain_deref()) {
        // The bound value rides out on a carrier witnessed by its binding scope's home frame, which
        // transitively pins that scope's reach-set — so the read names the value's reach by
        // construction rather than reconstructing it from the value.
        ValueCarrierResolution::Value(carrier) => Outcome::DoneWitnessed(carrier),
        ValueCarrierResolution::Placeholder(producer) => forward_to_producer(producer),
        ValueCarrierResolution::UnboundName => {
            Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name))))
        }
    }
}

pub(super) fn bare_type_leaf<'step, 'b>(
    ctx: &SchedulerView<'step, '_>,
    s: &'b Scope<'b>,
    t: &TypeIdentifier,
) -> Outcome<'step> {
    match resolve_type_leaf_carrier(s, t, ctx.active_chain()) {
        // A resolved type leaf seals under `s` (the scope it was resolved against): a `KType::Module`
        // folds its child-scope reach via `seal_type`, every owned / ancestor-pinned variant rides
        // `s`'s home frame — born co-located rather than bare-reattached to the step region.
        TypeLeafCarrier::Resolved(kt) => Outcome::DoneWitnessed(s.seal_type(Carried::Type(kt))),
        TypeLeafCarrier::Unbound(n) => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(n)))),
        // A still-finalizing referent. A visible type alias has already resolved its RHS
        // through the bridge, so a bare leaf parks on exactly one producer; park on it and
        // re-resolve once it seals. A producer already terminal-with-error short-circuits.
        TypeLeafCarrier::Park(producers) => {
            let producer = match producers.first() {
                Some(p) => *p,
                None => {
                    return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(t.render()))));
                }
            };
            if ctx.is_result_ready(producer) {
                if let Err(e) = ctx.read_result(producer) {
                    return Outcome::Done(Err(e.clone_for_propagation()));
                }
                // Ready-and-bound: the referent finalized between resolve and this check, so
                // re-resolve directly — the memoized bridge now admits.
                return bare_type_leaf(ctx, s, t);
            }
            // The producer's terminal is not the type carrier (a finalize-combine returns its own
            // value), so on wake `resume` re-resolves the leaf through the now-sealed memo rather
            // than lifting the producer's value. No spliced expression to render, so carrier is
            // `None`.
            let leaf = t.clone();
            park_resume(
                vec![producer],
                None,
                Box::new(move |ctx, _idx| {
                    ctx.with_current_scope(|s| bare_type_leaf(ctx, s, &leaf))
                }),
            )
        }
    }
}

pub(super) fn sigiled_type_expr<'step>(expr: KExpression<'step>) -> Outcome<'step> {
    let inner = match expr.parts.into_iter().next() {
        Some(Spanned {
            value: ExpressionPart::SigiledTypeExpr(boxed),
            ..
        }) => *boxed,
        _ => unreachable!("SigiledTypeExpr shape implies single SigiledTypeExpr part"),
    };
    become_dispatch(inner)
}

/// `:{x :Number, y :Str}` — a single-part record-type sigil. Folds the field list straight
/// to `Carried::Type(KType::Record(_))` via the shared field-list elaborator, deferring
/// through a dep-finish when a field forward-references or sub-dispatches. No type-constructor
/// builtin is involved — the record type is structural.
pub(super) fn record_type<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
    let fields = match expr.parts.into_iter().next() {
        Some(Spanned {
            value: ExpressionPart::RecordType(boxed),
            ..
        }) => *boxed,
        _ => unreachable!("RecordType shape implies a single RecordType part"),
    };
    let chain = ctx.current_lexical_chain();
    // The field-list elaborator is a pure decide: fold the structural record type now, or declare
    // its forward-ref/sub-dispatch deferral as a `ParkThenContinue`.
    super::field_list::elaborate_record_value(ctx, fields, chain)
}

/// `(99)`, `("x")`, `([1 2 3])`, `((inner))` etc. — single-part
/// literal-shaped expressions. Skips the bucket lookup + builtin call
/// the Keyworded path would otherwise route through.
pub(super) fn literal_pass_through<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
    let only = expr
        .parts
        .into_iter()
        .next()
        .expect("LiteralPassThrough shape implies one part");
    match only.value {
        // A literal is region-pure owned data, so the `KObject` is built **inside** the witness
        // closure — `yoke`d into this scope's frame, born co-located with that frame as its sole reach
        // rather than resolved at the ambient lifetime and bundled via `Witnessed::new`. (The literal
        // is scope-independent — it comes from `expr`, not a scope resolve — so it stays on the cart
        // region.)
        ExpressionPart::Literal(lit) => {
            let frame = ctx
                .current_scope()
                .region_owner()
                .upgrade()
                .expect("the dispatching scope's region owner is held for the step");
            let carrier = KoanRegion::alloc_witnessed(FrameSet::singleton(frame), move |region| {
                Carried::Object(region.alloc_object(lit.to_kobject()))
            });
            Outcome::DoneWitnessed(carrier)
        }
        ExpressionPart::Spliced(c) => Outcome::Done(Ok(c)),
        ExpressionPart::Expression(boxed) => become_dispatch(*boxed),
        ExpressionPart::ListLiteral(items) => park_on_literal(DepRequest::ListLit(items)),
        ExpressionPart::DictLiteral(pairs) => park_on_literal(DepRequest::DictLit(pairs)),
        ExpressionPart::RecordLiteral(fields) => park_on_literal(DepRequest::RecordLit(fields)),
        _ => unreachable!("LiteralPassThrough classifier only routes Literal/Spliced/Expression/ListLiteral/DictLiteral/RecordLiteral"),
    }
}

/// Park the slot on a single literal-producer dep as a [`Outcome::ParkThenContinue`] whose finish
/// lifts the producer's resolved value straight through. The harness submits the literal and owns
/// it; a dep error short-circuits frameless before the finish runs.
fn park_on_literal<'step>(dep: DepRequest<'step>) -> Outcome<'step> {
    let finish: DepFinish<'step> = Box::new(|_ctx, results, _carriers| Outcome::Done(Ok(results[0])));
    park_on_deps(vec![dep], None, finish)
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
pub(super) fn type_call<'step, 'b>(
    ctx: &SchedulerView<'step, '_>,
    s: &'b Scope<'b>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
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
    if let Resolution::Placeholder(producer) = s.resolve_with_chain(head_t.as_str(), chain) {
        if !ctx.is_result_ready(producer) {
            // The original call expression is the deadlock-summary sample; the resume re-runs
            // the whole fast lane against it once the head binding finalizes.
            let carrier = expr.summarize();
            return park_resume(
                vec![producer],
                Some(carrier),
                Box::new(move |ctx, _idx| ctx.with_current_scope(|s| type_call(ctx, s, expr))),
            );
        }
    }
    // Fresh `types[name]` lookup at construction time. A sealed nominal type's identity is
    // a `SetRef` whose member carries the schema (filled at the member's finalize) — no
    // value-side carrier involved. A bound functor lives here too, carrying its callable
    // body on `KType::KFunctor { body: Some(f) }`.
    let identity = match s.resolve_type_with_chain(head_t.as_str(), chain) {
        // Re-anchor the `'b`-branded type to the cart `'step` (it feeds `apply_callable`'s outcome).
        Some(kt) => {
            let Carried::Type(kt) = crate::scheduler::reattach_with::<CarriedFamily, _>(
                Carried::Type(kt),
                ctx.current_scope().region,
            ) else {
                unreachable!("reattach preserves the Type variant")
            };
            kt
        }
        None => {
            return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(head_t.render()))));
        }
    };
    match identity {
        // A bound functor's result is a module — the `Function` arm calls it.
        KType::KFunctor { body: Some(f), .. } => {
            apply_callable(ctx, ResolvedCallable::Function(f), &expr)
        }
        // A bare `:(FUNCTOR …)` type annotation has no callable to invoke.
        KType::KFunctor { body: None, .. } => {
            Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "constructible Type or bound functor".to_string(),
                got: identity.name(),
            })))
        }
        _ => apply_callable(ctx, ResolvedCallable::Constructor(identity), &expr),
    }
}
