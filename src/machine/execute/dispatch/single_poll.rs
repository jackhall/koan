//! Fast-lane dispatch shapes — bare identifier, bare leaf type,
//! bare-`Type`-head call, sigiled type expression, literal pass-through.
//! Most terminate (or single-producer-park) in one poll. `TypeCall` parks here on a
//! still-finalizing head binding, re-running [`type_call`] on wake — the per-value-cell eager
//! subs that follow a resolved head are the constructor dispatch's own `AwaitDeps` and resume
//! `finish_witnessed` there, not `type_call`. `BareTypeLeaf` parks on a still-finalizing referent
//! and re-resolves [`bare_type_leaf`]. Both single_poll parks route through a [`park_resume`]
//! closure.

use crate::machine::core::{FoldingBrand, KoanRegion, KoanRegionExt, Scope};
use crate::machine::model::{Carried, KObject};
use crate::machine::model::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::{KError, KErrorKind, NameLookup};
use crate::source::Spanned;

use super::super::lift::copy_carried;
use super::super::run_loop::{dest_brand, DestHandleFamily};
use super::super::StepCarried;
use super::super::WitnessedDepFinish;
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::SchedulerView;
use super::{
    become_dispatch, forward_to_producer, park_resume, type_channel, Await, DepRequest, Outcome,
    ProducerStanding, TypeChannel,
};
use crate::machine::model::CarriedFamily;
use crate::scheduler::Deps;
use crate::witnessed::Residence;

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
    // The leaf wants the raw resident carrier, not the sealed envelope, so it consumes the shared
    // type-channel + first-producer surface rather than the full sealing ladder.
    match type_channel(s, t, ctx.active_chain(), ctx.types()) {
        // A resolved type leaf is carried in place under `s` (the scope it was resolved
        // against): a `KType` is a `Copy` registry handle, so the read is a plain handle copy
        // — no reach to name, no re-home, no `child_scope()` walk.
        TypeChannel::Done(kt) => Outcome::Done(Ok(StepCarried::born(s.resident_type_carrier(kt)))),
        TypeChannel::Unbound(n) => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(n)))),
        // A still-finalizing referent. A visible type alias has already resolved its RHS through the
        // bridge, so a bare leaf parks on exactly one producer. A bare leaf has no consumer id in
        // scope, so its standing is read consumer-less — no cycle arm.
        TypeChannel::Parked(producer) => match ctx.producer_standing(producer) {
            ProducerStanding::Errored(e) => Outcome::Done(Err(e.clone_for_propagation())),
            // Ready-and-bound: the referent finalized between resolve and this check, so
            // re-resolve directly — the memoized bridge now admits.
            ProducerStanding::Ready => bare_type_leaf(ctx, s, t),
            // The producer's terminal is not the type carrier (a finalize-combine returns its own
            // value), so on wake `resume` re-resolves the leaf through the now-sealed memo rather
            // than lifting the producer's value. No spliced expression to render, so carrier is
            // `None`.
            ProducerStanding::Park => {
                let leaf = t.clone();
                park_resume(
                    vec![producer],
                    None,
                    Box::new(move |ctx, _idx| {
                        ctx.with_current_scope(|s| bare_type_leaf(ctx, s, &leaf))
                    }),
                )
            }
        },
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
    let chain = ctx.active_chain();
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
        // A quote is its body as data: seal the `KObject::KExpression` into this scope's region
        // through the **checked** door. `KExpression<'a>` is invariant with no `'static` rebuild,
        // so the family audit — which gates a `KExpression` by `is_splice_free` — runs as an
        // always-on loud gate, and a spliced cell surfaces as a structured error rather than an
        // assert. Parse output is splice-free, so the gate passes for every source quote.
        ExpressionPart::QuotedExpression(body) => Outcome::Done(
            ctx.current_scope()
                .brand()
                .alloc_object_witnessed_checked(KObject::KExpression(*body), ctx.types()),
        ),
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
/// classifies the identity, which routes through the shared apply-a-callable tail's
/// `Constructor` arm — a constructible `SetMember` identity (a sealed nominal type) is the
/// invocable case.
///
/// A `Parked` head (a still-finalizing `LET <Type-class> = …` binding, including a
/// recursive/forward type) parks on its producer and re-runs `type_call` on wake. A name
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
            // `UnboundName` since the resume re-runs the fast lane. No consumer id in scope, so the
            // standing is read consumer-less — no cycle arm.
            match ctx.producer_standing(producer) {
                ProducerStanding::Errored(_) | ProducerStanding::Ready => {
                    return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(
                        head_t.render(),
                    ))));
                }
                ProducerStanding::Park => {
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
    apply_callable(ctx, ResolvedCallable::Constructor { identity }, &expr)
}
