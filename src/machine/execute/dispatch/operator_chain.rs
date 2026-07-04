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
//! structured [`KErrorKind::DispatchFailed`]. A hit reaches the fold seam (precedence
//! climb + nested binary sub-dispatch), which is not yet implemented, so it terminates
//! with an explicit error rather than a silent fallthrough.

use crate::machine::core::Scope;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind};

use super::ctx::SchedulerView;
use super::Outcome;

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
            Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "operator-chain folding not yet implemented".to_string(),
            })))
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
