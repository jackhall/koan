//! Operator-chain dispatch arm: resolve the operator group for a
//! `Slot (Keyword Slot)+` chain.
//!
//! Recognition is structural and parse-cached (see
//! [`crate::machine::model::ast::classify_dispatch_shape`]); this arm resolves the
//! chain's cached operator probe against the per-scope operator registry, walked
//! through the scope chain (innermost visible wins, like every other name; see
//! [the lookup protocol](../../../../design/typing/lookup-protocol.md)).
//!
//! A miss — a cross-group operator mix, or an operator no module declared — surfaces a
//! structured [`KErrorKind::DispatchFailed`]. A hit reduces the run by the resolved group's
//! declared mode: [`ReductionMode::FoldLeft`] rewrites the chain into nested binary dispatches
//! (see [`reduce_fold_left`]), [`ReductionMode::FoldRight`] mirrors it right-associated (see
//! [`reduce_fold_right`]), [`ReductionMode::Unary`] rewrites it into one keyword-first call over
//! a list literal (see [`reduce_unary`]), and [`ReductionMode::Pairwise`] stages every operand as
//! its own dispatch and, once they all resolve, splices each result into the up-to-two adjacent
//! pairs it feeds before folding the pairs through the group's combiner in its declared direction
//! (see [`reduce_pairwise`] and [`combine`]) — the one mode that actually runs sub-dispatches
//! itself rather than purely rewriting syntax, since a shared middle operand must evaluate exactly
//! once.

use crate::machine::core::Scope;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::operators::{Combiner, FoldDirection, ReductionMode};
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind};
use crate::source::{Span, Spanned};

use super::ctx::SchedulerView;
use super::{become_dispatch, Outcome};

/// The probe is `Some` for every `OperatorChain` (the classifier guarantees it), so a
/// `None` probe is a classification bug.
///
/// Every path is terminal (no scheduler write), so this decides against a read-only
/// [`SchedulerView`] and returns [`Outcome::Done`].
pub(in crate::machine::execute) fn run<'step, 'b>(
    ctx: &SchedulerView<'step, '_>,
    s: &'b Scope<'b>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    let probe = expr
        .operator_probe()
        .expect("OperatorChain shape guarantees a cached operator probe");
    let chain = ctx.chain_deref();
    match s.resolve_operator_group_with_chain(probe, chain) {
        None => Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: undeclared_operator_reason(probe),
        }))),
        Some(group) => {
            // Guard against a registry-build bug: a hit whose probe operators aren't all
            // members surfaces as a clean cross-group non-match rather than a wrong fold.
            let operators = chain_operators(expr);
            if !group.covers(&operators) {
                return Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                    expr: expr.summarize(),
                    reason: cross_group_reason(probe),
                })));
            }
            match group.mode() {
                ReductionMode::FoldLeft => reduce_fold_left(ctx, expr),
                ReductionMode::FoldRight => reduce_fold_right(ctx, expr),
                ReductionMode::Unary => reduce_unary(ctx, expr),
                ReductionMode::Pairwise {
                    combiner,
                    direction,
                } => reduce_pairwise(ctx, expr, combiner.clone(), *direction),
            }
        }
    }
}

/// The operator keywords of the chain, in source order (with repeats).
fn chain_operators<'b>(expr: &'b KExpression<'_>) -> Vec<&'b str> {
    expr.parts
        .iter()
        .filter_map(|part| match &part.value {
            ExpressionPart::Keyword(s) => Some(s.as_str()),
            _ => None,
        })
        .collect()
}

/// Splits `expr.parts` into operands (even indices) and operator keywords (odd indices),
/// cloning each `Spanned` wrapper whole — not just the inner value — so source spans survive
/// into any error message the inner dispatch produces.
fn split_chain_parts<'step>(
    expr: &KExpression<'step>,
) -> (
    Vec<Spanned<ExpressionPart<'step>>>,
    Vec<Spanned<ExpressionPart<'step>>>,
) {
    let mut operands = Vec::with_capacity(expr.parts.len() / 2 + 1);
    let mut operator_keywords = Vec::with_capacity(expr.parts.len() / 2);
    for (i, part) in expr.parts.iter().enumerate() {
        if i % 2 == 0 {
            operands.push(part.clone());
        } else {
            operator_keywords.push(part.clone());
        }
    }
    (operands, operator_keywords)
}

/// Wraps a built-up accumulator as the next level's leading operand, carrying its own span
/// forward rather than inventing a fresh one.
fn wrap_as_operand<'step>(acc: KExpression<'step>) -> Spanned<ExpressionPart<'step>> {
    let span = acc.span;
    Spanned {
        value: ExpressionPart::Expression(Box::new(acc)),
        span,
    }
}

/// Rewrites a `FoldLeft`-mode run into nested binary dispatches — a pure syntactic rewrite,
/// since every operand appears exactly once there is no evaluation-order question:
///
/// `a + b + c` ⇒ `[ Expression([a, +, b]), +, c ]`, a bare 3-part expression whose nested
/// `Expression` operand resolves through the existing eager-subs sub-dispatch track before the
/// outer `+` runs as ordinary binary keyworded dispatch (the bodies `arithmetic::register`
/// installs). The outermost expression stays a bare 3-part expression — never itself wrapped in
/// `Expression(..)` — so [`become_dispatch`] re-enters ordinary dispatch on it directly.
fn reduce_fold_left<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let mut operands = operands.into_iter();
    let mut operators = operators.into_iter();

    let first_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let second_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let first_operator = operators
        .next()
        .expect("chain shape guarantees ≥2 operators");

    let mut acc = KExpression::new(vec![first_operand, first_operator, second_operand]);
    for (operator, operand) in operators.zip(operands) {
        acc = KExpression::new(vec![wrap_as_operand(acc), operator, operand]);
    }

    become_dispatch(ctx, acc)
}

/// Rewrites a `FoldRight`-mode run into nested binary dispatches — the mirror image of
/// [`reduce_fold_left`], nesting right-associated instead of left-associated:
///
/// `a - b - c` ⇒ `[ a, -, Expression([b, -, c]) ]`, a bare 3-part expression whose nested
/// `Expression` operand resolves through the existing eager-subs sub-dispatch track before the
/// outer `-` runs as ordinary binary keyworded dispatch. The outermost expression stays a bare
/// 3-part expression — never itself wrapped in `Expression(..)` — so [`become_dispatch`]
/// re-enters ordinary dispatch on it directly.
fn reduce_fold_right<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let mut operands = operands.into_iter().rev();
    let mut operators = operators.into_iter().rev();

    let last_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let second_last_operand = operands.next().expect("chain shape guarantees ≥3 operands");
    let last_operator = operators
        .next()
        .expect("chain shape guarantees ≥2 operators");

    let mut acc = KExpression::new(vec![second_last_operand, last_operator, last_operand]);
    for (operator, operand) in operators.zip(operands) {
        acc = KExpression::new(vec![operand, operator, wrap_as_operand(acc)]);
    }

    become_dispatch(ctx, acc)
}

/// Rewrites a `Unary`-mode run into one keyword-first call over a list literal — the prefix
/// surface `sym [x1 x2 x3]` and the infix chain `x1 sym x2 sym x3` are one dispatch shape for a
/// unary operator (the roadmap's "prefix and infix coincide" direction): both become the bare
/// 2-part expression `[ Keyword(sym), ListLiteral([x1, x2, x3]) ]`, the same shape `HEAD [1 2 3]`
/// dispatches through (a keyword-first call whose single slot carries a list). Unary is
/// homogeneous — a well-formed run names one operator throughout — so the first operator
/// keyword's span and text stand in for the whole run.
fn reduce_unary<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: &KExpression<'step>,
) -> Outcome<'step> {
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let operator = operators
        .into_iter()
        .next()
        .expect("chain shape guarantees ≥2 operators");
    let sym = match &operator.value {
        ExpressionPart::Keyword(s) => s.clone(),
        _ => unreachable!("odd-index chain parts are keywords by shape"),
    };
    let list_items: Vec<ExpressionPart<'step>> =
        operands.into_iter().map(|operand| operand.value).collect();
    let kw_part = Spanned {
        value: ExpressionPart::Keyword(sym),
        span: operator.span,
    };
    let list_part = Spanned {
        value: ExpressionPart::ListLiteral(list_items),
        span: expr.span,
    };
    become_dispatch(ctx, KExpression::new(vec![kw_part, list_part]))
}

/// Reduces a `Pairwise`-mode run: `f x < g y < h z` must evaluate `g y` **once**, its value
/// feeding both the `x<y` and `y<z` pairs, so — unlike the three modes above — this cannot be a
/// pure syntactic rewrite (each operand there appears exactly once in the output tree; here a
/// middle operand appears in two places). Every operand is staged as its own owned dep
/// (whatever its part kind — a bare identifier, a literal, or a parenthesized sub-expression all
/// dispatch through their normal lane via the one-part wrapper `install_pairwise_fold` builds);
/// once every operand resolves, the finish splices each resolved cell into the up-to-two pair
/// expressions it feeds (a `.duplicate()` per embed site) and folds the pairs through the group's
/// combiner in the declared direction. See [`SchedulerView::install_pairwise_fold`] for the
/// staging + finish mechanics (mirrors the shared eager-subs pattern in `ctx.rs`, but splices
/// into a fresh pair-tree rather than back into the original expression's own slots).
fn reduce_pairwise<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: &KExpression<'step>,
    combiner: Combiner,
    direction: FoldDirection,
) -> Outcome<'step> {
    let (operands, operators) = split_chain_parts(expr);
    debug_assert!(
        operands.len() >= 3 && operators.len() == operands.len() - 1,
        "OperatorChain shape guarantees ≥3 operands and one fewer operator"
    );
    let dep_error_frame = Some(crate::machine::TraceFrame::from_expr(
        "<operator-chain>",
        expr,
    ));
    ctx.install_pairwise_fold(
        operands,
        operators,
        combiner,
        direction,
        expr.span,
        dep_error_frame,
    )
}

/// The argument names a [`Combiner::Name`] call binds its two inputs to — the same pair an `OP`
/// body binds, so one naming rule covers the operator bodies and the combiner that folds their
/// results. A combiner function declaring other parameter names is an ordinary use-site error
/// (a missing argument), like any other call-by-name mismatch.
const COMBINER_LEFT: &str = "left";
const COMBINER_RIGHT: &str = "right";

/// One combiner application over two already-built sub-expressions — the fold step
/// [`SchedulerView::install_pairwise_fold`] repeats over a pairwise run's pair results. Both
/// combiner kinds produce the same *shape* one dispatch lane apart:
///
/// - [`Combiner::Keyword`] builds the 3-part keyworded expression `[left, <kw>, right]`, which
///   re-enters ordinary keyworded dispatch (the builtin comparison group's `AND`).
/// - [`Combiner::Name`] builds the 2-part call-by-name expression
///   `[Identifier(<name>), {left = …, right = …}]` — the `FunctionValueCall` lane, which resolves
///   `<name>` through the ordinary scope walk at the chain's *use site* (a group's `USING` window
///   surfaces the combiner alongside the operator bodies). A missing, non-callable, or
///   wrong-arity combiner therefore surfaces as an ordinary error there.
///
/// `span` labels the synthesized head parts, which have no source token of their own.
pub(super) fn combine<'step>(
    combiner: &Combiner,
    left: KExpression<'step>,
    right: KExpression<'step>,
    span: Option<Span>,
) -> KExpression<'step> {
    match combiner {
        Combiner::Keyword(keyword) => KExpression::new(vec![
            wrap_as_operand(left),
            Spanned {
                value: ExpressionPart::Keyword(keyword.clone()),
                span,
            },
            wrap_as_operand(right),
        ]),
        Combiner::Name(name) => KExpression::new(vec![
            Spanned {
                value: ExpressionPart::Identifier(name.clone()),
                span,
            },
            Spanned {
                value: ExpressionPart::RecordLiteral(vec![
                    (
                        COMBINER_LEFT.to_string(),
                        ExpressionPart::Expression(Box::new(left)),
                    ),
                    (
                        COMBINER_RIGHT.to_string(),
                        ExpressionPart::Expression(Box::new(right)),
                    ),
                ]),
                span,
            },
        ]),
    }
}

fn undeclared_operator_reason(probe: &str) -> String {
    format!(
        "no operator group declares all of `{probe}`; chainable operators must be \
         declared together in one module"
    )
}

fn cross_group_reason(probe: &str) -> String {
    format!(
        "operators `{probe}` span more than one operator group; chaining operators \
         across groups is disallowed"
    )
}
