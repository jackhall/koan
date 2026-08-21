//! Fast-lane dispatch shapes — bare identifier, bare leaf type, bare-`Type`-head call,
//! sigiled type expression, record type, literal pass-through. Most terminate (or park on a
//! single producer) in one poll.
//!
//! [`type_call`] and [`bare_type_leaf`] park on a still-finalizing binder through a
//! [`park_resume`] closure rather than a dep-finish: the wake has to re-run the resolution,
//! because the binder's terminal is not the value the lane wants.

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
use crate::witnessed::{BumpAllocator, Delivered};

/// Surfaces `UnboundName` directly when the name has no binding and
/// no visible placeholder — no dispatch retry, no overload search.
pub(super) fn bare_identifier<'step, 'b>(
    ctx: &DecideCtx<'_, 'step, '_>,
    s: &'b Scope<'b>,
    name: &str,
) -> Outcome<'step> {
    match s.resolve_value_delivered(name, ctx.chain_deref()) {
        // Lifted at its binding scope, so the carrier names the value's reach by construction
        // rather than reconstructing it from the value.
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
    match type_channel(s, &t, ctx.active_chain(), ctx.registries()) {
        // A `KType` is a `Copy` registry handle, so the leaf carries in place under the scope it
        // resolved against — no reach to name and no re-home.
        TypeChannel::Done(kt) => {
            Outcome::Done(Ok(StepCarried::born(s.resident(Carried::Type(kt)))))
        }
        TypeChannel::Unbound(n) => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(n)))),
        // The binder's terminal is not the type carrier (a finalize-combine returns its own
        // value), so the wake re-resolves the leaf against the now-sealed registry rather than
        // lifting that value.
        TypeChannel::Parked(source) => park_resume(
            vec![source],
            ctx.scratch(),
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

/// `:{x :Number, y :Str}` — a single-part record-type sigil. The record type is structural, so
/// no type-constructor builtin is involved: the shared field-list elaborator folds it, deferring
/// through a dep-finish when a field forward-references or sub-dispatches.
pub(super) fn record_type<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let fields = match expr.parts.first().map(|part| part.value) {
        Some(WorkingPart::Ast(ExpressionPart::RecordType(fields))) => {
            FieldParts::of(fields.reference())
        }
        // Same field list, read through the part family that can hold a resolved sibling handle —
        // a body whose co-declared references the declarator already threaded.
        Some(WorkingPart::RecordType(fields)) => FieldParts::threaded(fields),
        _ => unreachable!("RecordType shape implies a single RecordType part"),
    };
    let chain = ctx.active_chain();
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
        // A literal is region-pure — every borrow it carries points into this frame, a string
        // literal's bumped bytes included — so it is built inside a zero-dep fold, born
        // co-located with that frame as its sole reach.
        WorkingPart::Ast(ExpressionPart::Literal(lit)) => {
            let frame = ctx.dest_frame();
            let carrier = KoanRegion::fold_witnessed(frame, move |brand| {
                Carried::Object(brand.alloc_object_folded(lit.to_kobject(*brand)))
            });
            Outcome::Done(Ok(StepCarried::born_delivered(carrier)))
        }
        // A spliced cell already *is* the producer's own carrier: lifting the resting cell back
        // into a delivery envelope threads the exact reach its producer named, rather than
        // re-deriving one around a read-back value.
        WorkingPart::Spliced { cell } => Outcome::Done(Ok(StepCarried::born_delivered(
            ctx.current_scope().lift_spliced(&cell),
        ))),
        // A quote is its body as data. The AST it points at names no producer region, so the
        // carrier seals resident with no member.
        WorkingPart::Ast(ExpressionPart::QuotedExpression(body)) => Outcome::Done(Ok(ctx
            .current_scope()
            .brand()
            .alloc_expression_witnessed(body.expression()))),
        WorkingPart::Ast(ExpressionPart::Expression(inner)) => become_dispatch(
            ctx,
            WorkingExpression::from_ast(ctx.current_scope().brand(), *inner),
        ),
        WorkingPart::Expression(inner) => become_dispatch(ctx, *inner),
        WorkingPart::Ast(ExpressionPart::ListLiteral(items)) => {
            park_on_literal(DepRequest::ListLit(items), ctx.scratch())
        }
        WorkingPart::Ast(ExpressionPart::DictLiteral(pairs)) => {
            park_on_literal(DepRequest::DictLit(pairs), ctx.scratch())
        }
        WorkingPart::Ast(ExpressionPart::RecordLiteral(fields)) => {
            park_on_literal(DepRequest::RecordLit(fields), ctx.scratch())
        }
        _ => unreachable!(
            "LiteralPassThrough classifier only routes Literal/Spliced/Expression/QuotedExpression/ListLiteral/DictLiteral/RecordLiteral"
        ),
    }
}

/// Park the slot on a single literal-producer dep, whose finish relocates the produced value into
/// the consumer region — so the literal's reach rides the terminal by construction rather than
/// being recomputed beside it. A dep error short-circuits frameless before the finish runs.
fn park_on_literal<'step>(dep: DepRequest<'step>, scratch: BumpAllocator<'step>) -> Outcome<'step> {
    let finish: WitnessedDepFinish<'step> = Box::new(|view, deps| {
        // The destination is a bare region handle (empty reach) sealed as an envelope witnessed by
        // the consumer's own frame, so the seam composes the producer's reach into it and homes
        // the product there — co-located by construction rather than by an asserted singleton.
        Ok(StepCarried::born_delivered(relocate_seam(
            &view.current_scope().lift_spliced(&deps[0].cell),
            Delivered::destination(view.dest_frame()),
        )))
    });
    Await::on(Deps::from_requests_in([dep], scratch)).finish_witnessed(finish)
}

/// Bare-`Type`-head call. The head resolves on the type channel and the identity routes through
/// the shared apply-a-callable tail's `Constructor` arm, which decides the admitted body shape.
///
/// A `Parked` head (a still-finalizing `LET <Type-class> = …` binding, including a
/// recursive/forward type) parks on the binder's claim edge and re-runs on wake. A name with no
/// claim and no binding is `UnboundName`.
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
            // A finalized binder has already installed `types[name]`, so the `Bound` arm would
            // win; reaching here means the claim still stands.
            return park_resume(
                vec![source],
                ctx.scratch(),
                Box::new(move |ctx, _idx| type_call(ctx, expr)),
            );
        }
        None => {
            return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(head_t.render()))));
        }
    };
    apply_callable(ctx, ResolvedCallable::Constructor { identity }, &expr)
}
