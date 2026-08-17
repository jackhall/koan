//! Fast-lane dispatch shapes — bare identifier, bare leaf type,
//! bare-`Type`-head call, sigiled type expression, literal pass-through.
//! Most terminate (or single-producer-park) in one poll. `TypeCall` parks here on a
//! still-finalizing head binding, re-running [`type_call`] on wake — the per-value-cell eager
//! subs that follow a resolved head are the constructor dispatch's own `AwaitDeps` and resume
//! `finish_witnessed` there, not `type_call`. `BareTypeLeaf` parks on a still-finalizing referent
//! and re-resolves [`bare_type_leaf`]. Both single_poll parks route through a [`park_resume`]
//! closure.

use crate::machine::core::{KoanRegion, KoanRegionExt, Scope};
use crate::machine::model::Carried;
use crate::machine::model::FieldParts;
use crate::machine::model::{ExpressionPart, TypeIdentifier, WorkingExpression, WorkingPart};
use crate::machine::{KError, KErrorKind, NameLookup};

use super::super::StepCarried;
use super::super::WitnessedDepFinish;
use super::super::lift::relocate_seam;
use super::apply_callable::{ResolvedCallable, apply_callable};
use super::ctx::DecideCtx;
use super::{
    Await, DepRequest, Outcome, TypeChannel, become_dispatch, forward_to_producer, park_resume,
    type_channel,
};
use crate::scheduler::Deps;
use crate::witnessed::Delivered;

/// Surfaces `UnboundName` directly when the name has no binding and
/// no visible placeholder — no dispatch retry, no overload search.
pub(super) fn bare_identifier<'step, 'b>(
    ctx: &DecideCtx<'_, 'step, '_>,
    s: &'b Scope<'b>,
    name: &str,
) -> Outcome<'step> {
    match s.resolve_value_delivered(name, ctx.chain_deref()) {
        // The bound value rides out on a carrier lifted at its binding scope, pinned by that
        // scope's own region owner — so the read names the value's reach by construction rather
        // than reconstructing it from the value.
        Some(NameLookup::Bound(delivered)) => {
            Outcome::Done(Ok(StepCarried::born_delivered(delivered)))
        }
        Some(NameLookup::Parked(source)) => forward_to_producer(source),
        None => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name.to_string())))),
    }
}

pub(super) fn bare_type_leaf<'step, 'b>(
    ctx: &DecideCtx<'_, 'step, '_>,
    s: &'b Scope<'b>,
    t: TypeIdentifier<'step>,
) -> Outcome<'step> {
    // The leaf wants the raw resident carrier, not the sealed envelope, so it consumes the shared
    // type-channel + first-source surface rather than the full sealing ladder.
    match type_channel(s, &t, ctx.active_chain(), ctx.types()) {
        // A resolved type leaf is carried in place under `s` (the scope it was resolved
        // against): a `KType` is a `Copy` registry handle, so the read is a plain handle copy
        // — no reach to name, no re-home, no `child_scope()` walk.
        TypeChannel::Done(kt) => {
            Outcome::Done(Ok(StepCarried::born(s.resident(Carried::Type(kt)))))
        }
        TypeChannel::Unbound(n) => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(n)))),
        // A still-finalizing referent. A visible type alias has already resolved its RHS through the
        // bridge, so a bare leaf parks on exactly one binder edge. The binder's terminal is not the
        // type carrier (a finalize-combine returns its own value), so on wake `resume` re-resolves
        // the leaf through the now-sealed memo rather than lifting that value; a binder that
        // errored propagates through the park the harness installs. No spliced expression to
        // render, so carrier is `None`.
        TypeChannel::Parked(source) => park_resume(
            vec![source],
            Box::new(move |ctx, _idx| bare_type_leaf(ctx, ctx.current_scope(), t)),
        ),
    }
}

pub(super) fn sigiled_type_expr<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let inner = match expr.parts.first().map(|part| part.value) {
        Some(WorkingPart::Ast(ExpressionPart::SigiledTypeExpr(inner))) => *inner,
        _ => unreachable!("SigiledTypeExpr shape implies single SigiledTypeExpr part"),
    };
    become_dispatch(
        ctx,
        WorkingExpression::from_ast(ctx.current_scope().brand(), inner),
    )
}

/// `:{x :Number, y :Str}` — a single-part record-type sigil. Folds the field list straight
/// to `Carried::Type(KType::Record { .. })` via the shared field-list elaborator, deferring
/// through a dep-finish when a field forward-references or sub-dispatches. No type-constructor
/// builtin is involved — the record type is structural.
pub(super) fn record_type<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let fields = match expr.parts.first().map(|part| part.value) {
        Some(WorkingPart::Ast(ExpressionPart::RecordType(fields))) => {
            FieldParts::of(fields.reference())
        }
        // A body whose co-declared references the declarator already threaded — same field list,
        // read through the part family that can hold a resolved sibling handle.
        Some(WorkingPart::RecordType(fields)) => FieldParts::threaded(fields),
        _ => unreachable!("RecordType shape implies a single RecordType part"),
    };
    let chain = ctx.active_chain();
    // The field-list elaborator is a pure decide: fold the structural record type now, or declare
    // its forward-ref/sub-dispatch deferral as a `Park`.
    super::field_list::elaborate_record_value(ctx, fields, chain)
}

/// `(99)`, `("x")`, `([1 2 3])`, `((inner))` etc. — single-part
/// literal-shaped expressions. Skips the bucket lookup + builtin call
/// the Keyworded path would otherwise route through.
pub(super) fn literal_pass_through<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let only = expr
        .parts
        .first()
        .expect("LiteralPassThrough shape implies one part");
    match only.value {
        // A literal is region-pure — every borrow it carries points into this scope's own frame, a
        // string literal's bumped bytes included — so the `KObject` is built inside a zero-dep fold,
        // born co-located with that frame as its sole reach. It comes from `expr`, not a scope
        // resolve, so it stays on the cart region.
        WorkingPart::Ast(ExpressionPart::Literal(lit)) => {
            let frame = ctx.dest_frame();
            let carrier = KoanRegion::fold_witnessed(frame, move |brand| {
                Carried::Object(brand.alloc_object_folded(lit.to_kobject(*brand)))
            });
            Outcome::Done(Ok(StepCarried::born_delivered(carrier)))
        }
        // A spliced cell already *is* the producer's own carrier — recover it directly with `unseal`
        // rather than re-wrapping the read-back value under a freshly-asserted witness. Strictly
        // better witnessing: the value arrives with the exact reach its producer named.
        WorkingPart::Spliced { cell } => {
            // Lift the resting cell back into its producer's own delivery envelope under the step's
            // coverage: the whole claim — the producer's own region among its members — is re-owned
            // there, so the recovered carrier's reach is threaded, not re-derived.
            Outcome::Done(Ok(StepCarried::born_delivered(
                ctx.current_scope().lift_spliced(&cell),
            )))
        }
        // A quote is its body as data: bump the `KObject::KExpression` into this scope's region
        // through the door whose signature admits an expression and nothing else. The value is
        // invariant in its region lifetime with no `'static` rebuild, and the AST it points at names
        // no producer region, so the carrier seals resident with no member.
        WorkingPart::Ast(ExpressionPart::QuotedExpression(body)) => Outcome::Done(Ok(ctx
            .current_scope()
            .brand()
            .alloc_expression_witnessed(body.expression()))),
        WorkingPart::Ast(ExpressionPart::Expression(inner)) => become_dispatch(
            ctx,
            WorkingExpression::from_ast(ctx.current_scope().brand(), *inner),
        ),
        // A node the scheduler synthesized dispatches in place — it is already working form.
        WorkingPart::Expression(inner) => become_dispatch(ctx, *inner),
        WorkingPart::Ast(ExpressionPart::ListLiteral(items)) => {
            park_on_literal(DepRequest::ListLit(items))
        }
        WorkingPart::Ast(ExpressionPart::DictLiteral(pairs)) => {
            park_on_literal(DepRequest::DictLit(pairs))
        }
        WorkingPart::Ast(ExpressionPart::RecordLiteral(fields)) => {
            park_on_literal(DepRequest::RecordLit(fields))
        }
        _ => unreachable!(
            "LiteralPassThrough classifier only routes Literal/Spliced/Expression/ListLiteral/DictLiteral/RecordLiteral"
        ),
    }
}

/// Park the slot on a single literal-producer dep as an [`Outcome::Park`] whose finish folds the
/// producer's carrier into this slot's own witnessed terminal — relocating the value into the
/// consumer region (`transfer_into`) and naming its reach on the carrier, so the literal's reach
/// rides the terminal by construction rather than being recomputed beside it. The harness submits the
/// literal and owns it; a dep error short-circuits frameless before the finish runs.
fn park_on_literal<'step>(dep: DepRequest<'step>) -> Outcome<'step> {
    let finish: WitnessedDepFinish<'step> = Box::new(|view, deps| {
        // The destination operand is `view.dest_frame()`'s own region handle, sealed as an envelope
        // witnessed by it — co-located by construction rather than paired with an asserted
        // singleton. It is a bare region handle (empty reach); the seam composes the literal
        // producer's reach into it and homes the product in the consumer's own frame, which the
        // step's seal re-pins — so `born_delivered` releases it and the foreign coverage rides on.
        Ok(StepCarried::born_delivered(relocate_seam(
            &view.current_scope().lift_spliced(&deps[0].cell),
            Delivered::destination(view.dest_frame()),
        )))
    });
    Await::on(Deps::from_requests([dep])).finish_witnessed(finish)
}

/// Bare-`Type`-head call. A single `resolve_type_with_chain` (a `types[name]` read)
/// classifies the identity, which routes through the shared apply-a-callable tail's
/// `Constructor` arm — a constructible `SetMember` identity (a sealed nominal type) is the
/// invocable case.
///
/// A `Parked` head (a still-finalizing `LET <Type-class> = …` binding, including a
/// recursive/forward type) parks on the binder's claim edge and re-runs `type_call` on wake. A name
/// with no claim and no binding is `UnboundName`.
pub(super) fn type_call<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let head_t = match expr.parts[0].value {
        WorkingPart::Ast(ExpressionPart::Type(t)) => t,
        _ => unreachable!("TypeCall shape implies leaf Type head"),
    };
    let chain = ctx.chain_deref();
    // Resolve against the cart scope at `'step`, so the resolved identity rides into the outcome
    // with no re-anchor.
    let scope = ctx.current_scope();
    let identity = match scope.resolve_type_with_chain(head_t.as_str(), chain) {
        Some(NameLookup::Bound(kt)) => kt,
        Some(NameLookup::Parked(source)) => {
            // A finalized binder has already installed `types[name]`, so the `Bound` arm would win;
            // reaching here means the claim still stands, so park on it and re-run the fast lane on
            // wake — where either the identity is bound or the name is genuinely `UnboundName`.
            return park_resume(
                vec![source],
                Box::new(move |ctx, _idx| type_call(ctx, expr)),
            );
        }
        None => {
            return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(head_t.render()))));
        }
    };
    apply_callable(ctx, ResolvedCallable::Constructor { identity }, &expr)
}
