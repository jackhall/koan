//! Operator-chain dispatch arm: resolve the operator group for a
//! `Slot (Keyword Slot)+` chain, then hand off to the fold pre-pass.
//!
//! Recognition is structural and parse-cached (see
//! [`crate::machine::model::ast::classify_dispatch_shape`]); this arm performs the
//! *resolution* step — looking the chain's cached operator probe up in the per-scope
//! operator registry, walked through the scope chain (innermost visible wins, like
//! every other name; see [the lookup protocol](../../../../design/typing/lookup-protocol.md)).
//!
//! A miss — a cross-group operator mix, or an operator no module declared — surfaces a
//! structured [`KErrorKind::DispatchFailed`]. A hit reaches the fold seam: the fold
//! itself (precedence climb + nested binary sub-dispatch) is the follow-on increment,
//! so the hit currently terminates with an explicit "not yet implemented" rather than
//! a silent fallthrough. In production the registry is empty until the `OP` binder
//! lands, so every chain misses and errors cleanly; the hit path is exercised only by
//! test fixtures that register an `OperatorGroup`.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind};

use super::super::nodes::NodeOutput;
use super::ctx::DispatchCx;
use super::outcome::DispatchOutcome;

/// Resolve the chain's operator group via the cached probe and route to the fold
/// seam. The probe is `Some` for every `OperatorChain` (the classifier guarantees it),
/// so a `None` probe is a classification bug.
///
/// This handler issues no scheduler write — every path is a terminal — so it decides
/// against a read-only [`DispatchCx`] and returns a [`DispatchOutcome::Terminal`]; the
/// router applies it through [`super::harness::apply_dispatch_outcome`].
pub(in crate::machine::execute) fn run<'run>(
    ctx: &DispatchCx<'run, '_>,
    expr: &KExpression<'run>,
) -> DispatchOutcome<'run> {
    let probe = expr
        .operator_probe()
        .expect("OperatorChain shape guarantees a cached operator probe");
    let chain = ctx.chain_deref();
    match ctx
        .current_scope()
        .resolve_operator_group_with_chain(probe, chain)
    {
        None => {
            DispatchOutcome::Terminal(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: undeclared_operator_reason(probe),
            })))
        }
        Some(group) => {
            // A hit on a key whose probe operators aren't all members would be a
            // registry-build bug (the powerset keys only name members), but guard it:
            // a mismatch is a cross-group mix surfacing as a clean non-match.
            let operators = chain_operators(expr);
            if !group.covers(&operators) {
                return DispatchOutcome::Terminal(NodeOutput::Err(KError::new(
                    KErrorKind::DispatchFailed {
                        expr: expr.summarize(),
                        reason: cross_group_reason(probe),
                    },
                )));
            }
            // Fold seam: the precedence climb + binary sub-dispatch is the follow-on.
            DispatchOutcome::Terminal(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
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
