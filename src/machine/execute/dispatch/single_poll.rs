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
use crate::machine::model::TypeResolution;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::model::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::{KError, KErrorKind, NameLookup};
use crate::source::Spanned;

use super::super::lift::copy_carried;
use super::super::run_loop::{dest_brand, DestHandleFamily};
use super::super::StepCarried;
use super::super::WitnessedDepFinish;
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::SchedulerView;
use super::{become_dispatch, forward_to_producer, park_resume, Await, DepRequest, Outcome};
use crate::machine::model::CarriedFamily;
use crate::scheduler::{Deps, ProducerDisposition};
use crate::witnessed::Residence;

/// Schema-keyed payload the resume needs to materialize the constructed value once every
/// slot is resolved. `identity` / `constructor` is the sealed member's handle, stamped onto the
/// produced `KObject`; `schema` is the member's variant schema, used for per-value type-checking.
pub(in crate::machine::execute) enum CtorKind<'step> {
    /// NewType construction (record-repr or scalar) from a single positional value. One value
    /// cell carrying the whole value expression; the finish type-checks it against the
    /// member's `repr`, peels any `Wrapped` layer, and tags it with `identity`.
    NewType { identity: &'step KType },
    /// Record-repr newtype construction from a named record-literal body (`Point {x = 1, y =
    /// 2}`). One value cell per field, so a literal field stages in place (synchronous bind)
    /// instead of deferring the whole record literal; the
    /// finish builds the `KObject::Record` and wraps it with `identity`.
    RecordNewType {
        identity: &'step KType,
        field_names: Vec<String>,
    },
    Tagged {
        schema: Rc<HashMap<String, KType>>,
        /// The sealed union member's own handle — what the built `Tagged` carries as its
        /// `identity`, and what its `ktype()` reports.
        member: KType,
        tag: String,
    },
    /// Identity-wrapper construction over a `NEWTYPE (Type AS Wrapper)`-declared constructor
    /// family (empty-schema `TypeConstructor` member). One value cell carrying the whole value
    /// expression; the finish stamps the value's full type as the sole applied arg, peels any
    /// `Wrapped` layer, and wraps the payload with a fresh
    /// `ConstructorApply(Wrapper, {<param> = <arg>})`
    /// type id — so the built value inhabits `:(<v's type> AS Wrapper)`.
    ApplyConstructor { constructor: KType },
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
    match s.resolve_type_identifier(t, ctx.active_chain(), ctx.types()) {
        // A resolved type leaf is witnessed in place under `s` (the scope it was resolved
        // against): a `KType` is owned data, so the read travels under `s`'s home-frame pin
        // alone — no reach to name, no `alloc_ktype` re-home, no `child_scope()` walk.
        TypeResolution::Done(kt) => {
            Outcome::Done(Ok(StepCarried::born(s.resident_type_carrier(kt))))
        }
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
    apply_callable(ctx, ResolvedCallable::Constructor { identity }, &expr)
}
