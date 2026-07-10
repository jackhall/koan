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
//! [`reduce_fold_right`]), and [`ReductionMode::Unary`] rewrites it into one keyword-first call
//! over a list literal (see [`reduce_unary`]); all three hand control back to dispatch. The
//! remaining mode ([`ReductionMode::Pairwise`]) still terminates at the explicit "not yet
//! implemented" seam.

use crate::machine::core::Scope;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::operators::ReductionMode;
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind};
use crate::source::Spanned;

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
                ReductionMode::Pairwise { .. } => {
                    Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                        expr: expr.summarize(),
                        reason: "operator-chain folding not yet implemented".to_string(),
                    })))
                }
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
