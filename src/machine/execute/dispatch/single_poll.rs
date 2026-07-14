//! Fast-lane dispatch shapes — bare identifier, bare leaf type,
//! bare-`Type`-head call, sigiled type expression, literal pass-through.
//! Most terminate (or single-producer-park) in one poll. Two carry a resume:
//! `TypeCall` parks on per-value-cell eager-subs (its value subs as a
//! `AwaitDeps`) or on a still-finalizing head binding, re-running
//! [`type_call`] on wake; `BareTypeLeaf` parks on a still-finalizing referent
//! and re-resolves [`bare_type_leaf`]. Both park through a [`park_resume`] closure.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{FoldingBrand, KoanRegion, KoanRegionExt, Scope};
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::types::TypeResolution;
use crate::machine::model::{Carried, KType, Parseable, RecursiveSet};
use crate::machine::{KError, KErrorKind, NameLookup};
use crate::source::Spanned;

use super::super::lift::copy_carried;
use super::super::run_loop::{dest_brand, DestHandleFamily};
use super::super::StepCarried;
use super::super::WitnessedDepFinish;
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::SchedulerView;
use super::{become_dispatch, forward_to_producer, park_resume, Await, DepRequest, Outcome};
use crate::machine::core::StoredReach;
use crate::machine::model::values::CarriedFamily;
use crate::scheduler::{Deps, ProducerDisposition};
use crate::witnessed::Residence;

/// Schema-keyed payload the resume needs to materialize the constructed value once every
/// slot is resolved. `(set, index)` is the sealed-member identity stamped onto the produced
/// `KObject`; `schema` is the projected (sibling-`SetLocal`-resolved) schema used for
/// per-value type-checking.
pub(in crate::machine::execute) enum CtorKind<'step> {
    /// NewType construction (record-repr or scalar) from a single positional value. One value
    /// cell carrying the whole value expression; the finish type-checks it against the
    /// member's `repr`, peels any `Wrapped` layer, and tags it with `identity`. `reach` is the
    /// identity's stored per-binding type reach, folded into the construction operand's witness so
    /// it names the identity's own region.
    NewType {
        identity: &'step KType<'step>,
        reach: StoredReach<'step>,
    },
    /// Record-repr newtype construction from a named record-literal body (`Point {x = 1, y =
    /// 2}`). One value cell per field, so a literal field stages in place (synchronous bind)
    /// instead of deferring the whole record literal; the
    /// finish builds the `KObject::Record` and wraps it with `identity`. `reach` carries the
    /// identity's stored per-binding type reach for the construction operand's witness.
    RecordNewType {
        identity: &'step KType<'step>,
        field_names: Vec<String>,
        reach: StoredReach<'step>,
    },
    Tagged {
        schema: Rc<HashMap<String, KType<'step>>>,
        set: Rc<RecursiveSet<'step>>,
        index: usize,
        tag: String,
        /// The identity's stored per-binding type token, folded into the construction operand's
        /// witness. The `Tagged` identity is a fresh dest-region `SetRef`, so `reach` is empty
        /// today; it names the set's region once `RecursiveSet` is region-allocated.
        reach: StoredReach<'step>,
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
        Some(NameLookup::Bound(carrier)) => Outcome::Done(Ok(StepCarried::born(carrier))),
        Some(NameLookup::Parked(producer)) => forward_to_producer(producer),
        None => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name)))),
    }
}

pub(super) fn bare_type_leaf<'step, 'b>(
    ctx: &SchedulerView<'step, '_>,
    s: &'b Scope<'b>,
    t: &TypeIdentifier,
) -> Outcome<'step> {
    match s.resolve_type_identifier(t, ctx.active_chain()) {
        // A resolved type leaf is witnessed in place under `s` (the scope it was resolved against) from
        // its binding's stored `reach`: `s`'s home frame pins the type's own / ancestor region, and
        // `reach` names any genuinely-foreign region (a module's child scope) — no `alloc_ktype`
        // re-home, no `child_scope()` walk.
        TypeResolution::Done(resolved) => Outcome::Done(Ok(StepCarried::born(
            s.resident_type_carrier(resolved.kt, resolved.stored),
        ))),
        TypeResolution::Unbound(n) => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(n)))),
        // A still-finalizing referent. A visible type alias has already resolved its RHS
        // through the bridge, so a bare leaf parks on exactly one producer; park on it and
        // re-resolve once it seals. A producer already terminal-with-error short-circuits.
        TypeResolution::Park(producers) => {
            let producer = match producers.first() {
                Some(p) => *p,
                None => {
                    return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(t.render()))));
                }
            };
            // A bare leaf has no consumer id in scope, so the disposition can never classify `Cycle`.
            match ctx.producer_disposition(producer, None) {
                ProducerDisposition::Errored(e) => Outcome::Done(Err(e.clone_for_propagation())),
                // Ready-and-bound: the referent finalized between resolve and this check, so
                // re-resolve directly — the memoized bridge now admits.
                ProducerDisposition::Ready => bare_type_leaf(ctx, s, t),
                ProducerDisposition::Cycle => {
                    unreachable!("bare_type_leaf passes consumer=None, so Cycle never classifies")
                }
                // The producer's terminal is not the type carrier (a finalize-combine returns its own
                // value), so on wake `resume` re-resolves the leaf through the now-sealed memo rather
                // than lifting the producer's value. No spliced expression to render, so carrier is
                // `None`.
                ProducerDisposition::Park => {
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
    }
}

pub(super) fn sigiled_type_expr<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
    let inner = match expr.parts.into_iter().next() {
        Some(Spanned {
            value: ExpressionPart::SigiledTypeExpr(boxed),
            ..
        }) => *boxed,
        _ => unreachable!("SigiledTypeExpr shape implies single SigiledTypeExpr part"),
    };
    become_dispatch(ctx, inner)
}

/// `:{x :Number, y :Str}` — a single-part record-type sigil. Folds the field list straight
/// to `Carried::Type(KType::Record { .. })` via the shared field-list elaborator, deferring
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
        // A literal is region-pure owned data, so the `KObject` is built inside the witness closure
        // — `yoke`d into this scope's frame, born co-located with it as its sole reach. It comes from
        // `expr`, not a scope resolve, so it stays on the cart region.
        ExpressionPart::Literal(lit) => {
            let frame = ctx.dest_frame();
            let carrier = KoanRegion::alloc_witnessed(frame, move |region| {
                Carried::Object(region.alloc_object(lit.to_kobject()))
            });
            Outcome::Done(Ok(StepCarried::born(carrier)))
        }
        // A spliced cell already *is* the producer's own carrier — recover it directly with `unseal`
        // rather than re-wrapping the read-back value under a freshly-asserted witness. Strictly
        // better witnessing: the value arrives with the exact reach its producer named.
        ExpressionPart::Spliced { cell } => {
            Outcome::Done(Ok(StepCarried::born(cell.into_cell().unseal())))
        }
        ExpressionPart::Expression(boxed) => become_dispatch(ctx, *boxed),
        ExpressionPart::ListLiteral(items) => park_on_literal(DepRequest::ListLit(items)),
        ExpressionPart::DictLiteral(pairs) => park_on_literal(DepRequest::DictLit(pairs)),
        ExpressionPart::RecordLiteral(fields) => park_on_literal(DepRequest::RecordLit(fields)),
        _ => unreachable!("LiteralPassThrough classifier only routes Literal/Spliced/Expression/ListLiteral/DictLiteral/RecordLiteral"),
    }
}

/// Park the slot on a single literal-producer dep as a [`Outcome::ParkThenContinue`] whose finish
/// folds the producer's carrier into this slot's own witnessed terminal — relocating the value into
/// the consumer region (`transfer_into`) and naming its reach on the carrier, so the literal's reach
/// rides the terminal by construction rather than being recomputed beside it. The harness submits the
/// literal and owns it; a dep error short-circuits frameless before the finish runs.
fn park_on_literal<'step>(dep: DepRequest<'step>) -> Outcome<'step> {
    let finish: WitnessedDepFinish<'step> = Box::new(|view, deps| {
        // The dest brand is `yoke`d into the frame that owns the consumer scope's region, witnessed by
        // it — co-located by construction rather than paired with an asserted singleton.
        let dest = dest_brand(view.dest_frame());
        Ok(StepCarried::born(
            deps.owned(0)
                .delivered
                .transfer_into_placing::<DestHandleFamily, CarriedFamily, _>(
                    dest,
                    Residence::Copied,
                    |value, _region, placement| {
                        copy_carried(value, FoldingBrand::in_fold_closure(placement))
                    },
                ),
        ))
    });
    Await::on(Deps::from_owned([dep])).finish_witnessed(finish)
}

/// Bare-`Type`-head call. A single `resolve_type_with_chain` (a `types[name]` read)
/// classifies the identity, and the branched outcomes route through the shared
/// apply-a-callable tail:
/// - a constructible `SetRef` identity (a sealed nominal type) → the `Constructor` arm;
/// - a `KType::KFunctor { body: Some(f) }` (a bound functor) → the `Function` arm — its
///   result is a module;
/// - a `KType::KFunctor { body: None }` (a bare `:(FUNCTOR …)` annotation) is not invocable
///   → `TypeMismatch`.
///
/// A `Parked` head (a still-finalizing `LET <Type-class> = …` binding, including a
/// recursive/forward functor) parks on its producer and re-runs `type_call` on wake. A name
/// with no producer and no binding is `UnboundName`.
pub(super) fn type_call<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
    let head_t = match &expr.parts[0].value {
        ExpressionPart::Type(t) => t.clone(),
        _ => unreachable!("TypeCall shape implies leaf Type head"),
    };
    let chain = ctx.chain_deref();
    // Resolve against the cart scope at `'step`, so the resolved identity rides into the outcome
    // with no re-anchor.
    let scope = ctx.current_scope();
    let identity = match scope.resolve_type_with_chain(head_t.as_str(), chain) {
        Some(NameLookup::Bound(kt)) => kt,
        Some(NameLookup::Parked(producer)) => {
            // A terminal producer has already installed `types[name]`, so the `Bound` arm would win;
            // reaching here with one (Ready or errored) means a mid-write/errored binder, surfaced as
            // `UnboundName` since the resume re-runs the fast lane. No consumer id in scope, so `Cycle`
            // never classifies.
            match ctx.producer_disposition(producer, None) {
                ProducerDisposition::Errored(_) | ProducerDisposition::Ready => {
                    return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(
                        head_t.render(),
                    ))));
                }
                ProducerDisposition::Cycle => {
                    unreachable!("type_call passes consumer=None, so Cycle never classifies")
                }
                ProducerDisposition::Park => {
                    let carrier = expr.summarize();
                    return park_resume(
                        vec![producer],
                        Some(carrier),
                        Box::new(move |ctx, _idx| type_call(ctx, expr)),
                    );
                }
            }
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
        _ => {
            // The identity's stored per-binding type token (home-omitted foreign reach + home-borrow
            // bit), resolved through the same lexical chain as the identity: threaded to the
            // construction finish so its operand names the identity's own region rather than relying
            // on the dest frame's storage `outer` chain, which omits lexical ancestors under TCO.
            // Empty while `RecursiveSet` is heap-`Rc`'d.
            let reach = scope.type_reach(head_t.as_str(), chain);
            apply_callable(
                ctx,
                ResolvedCallable::Constructor { identity, reach },
                &expr,
            )
        }
    }
}
