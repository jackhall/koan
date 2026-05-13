//! Signature parsing for the `FN` builtin.
//!
//! Two entry points:
//! - [`parse_fn_param_list`] — the full structural parse used by [`super::body`] at FN
//!   construction time. Returns a `ParamListOutcome` so the caller can route through a
//!   `Combine` when one or more parameter-type names resolve to a pending placeholder.
//! - [`pre_run`] — the dispatch-time placeholder extractor used by `register` to
//!   announce the function's name before its body runs.

use crate::runtime::model::{Argument, SignatureElement};
use crate::runtime::model::types::{elaborate_type_expr, ElabResult, Elaborator};
use crate::runtime::machine::NodeId;
use crate::ast::{ExpressionPart, KExpression};

/// Result of one walk over an FN signature's part list.
pub(super) enum ParamListOutcome {
    /// Every parameter type elaborated against the captured scope; the resulting
    /// `SignatureElement`s are ready to bind.
    Done(Vec<SignatureElement>),
    /// One or more parameter-type leaf names resolved to scheduler placeholders that
    /// hadn't finalized at FN-def time. The caller schedules a `Combine` over
    /// `producers`; when every producer terminalizes, the closure re-runs
    /// `parse_fn_param_list` against the (now-final) scope and finalizes the FN.
    Park(Vec<NodeId>),
    /// A structural / unbound / cycle error surfaced during elaboration. The caller wraps
    /// in `ShapeError`.
    Err(String),
}

/// Convert the captured FN-parameter-list `KExpression` into a list of `SignatureElement`s.
/// Walks the parts left-to-right, consuming bare `Keyword` parts as fixed tokens and
/// `Identifier(name) Keyword(":") Type(t)` triples as typed `Argument` slots. Stray `:`,
/// stray `Type`, missing `: Type` annotations, and other malformed shapes surface as
/// `ParamListOutcome::Err(msg)`.
///
/// Type-name resolution rides on the scheduler-aware [`elaborate_type_expr`]: it consults
/// the captured scope's `placeholders` map alongside its `data` map, returning
/// `ElabResult::Park(producers)` for type-binding names that have dispatched but not
/// finalized. Multiple parking producers accumulate across the whole signature walk so the
/// caller can register every blocker in one Combine.
pub(super) fn parse_fn_param_list<'a>(
    signature: &KExpression<'a>,
    elaborator: &mut Elaborator<'_, '_>,
) -> ParamListOutcome {
    let parts = &signature.parts;
    let mut elements: Vec<SignatureElement> = Vec::with_capacity(parts.len());
    let mut parks: Vec<NodeId> = Vec::new();
    let mut first_err: Option<String> = None;
    let mut i = 0;
    while i < parts.len() {
        match &parts[i] {
            ExpressionPart::Keyword(s) if s == ":" => {
                return ParamListOutcome::Err(
                    "FN signature has a stray `:` outside a `<name>: <Type>` triple".to_string(),
                );
            }
            ExpressionPart::Keyword(s) => {
                elements.push(SignatureElement::Keyword(s.clone()));
                i += 1;
            }
            ExpressionPart::Identifier(name) => {
                let colon = parts.get(i + 1);
                let ty = parts.get(i + 2);
                match (colon, ty) {
                    (Some(ExpressionPart::Keyword(c)), Some(ExpressionPart::Type(t))) if c == ":" => {
                        match elaborate_type_expr(elaborator, t) {
                            ElabResult::Done(kt) => {
                                elements.push(SignatureElement::Argument(Argument {
                                    name: name.clone(),
                                    ktype: kt,
                                }));
                            }
                            ElabResult::Park(producers) => {
                                parks.extend(producers);
                            }
                            ElabResult::Unbound(msg) if first_err.is_none() => {
                                first_err = Some(format!(
                                    "{msg} in FN signature for parameter `{name}`"
                                ));
                            }
                            ElabResult::Unbound(_) => {}
                        }
                        i += 3;
                    }
                    _ => {
                        return ParamListOutcome::Err(format!(
                            "FN signature parameter `{name}` requires a `: Type` annotation \
                             (e.g. `{name}: Number`)",
                        ));
                    }
                }
            }
            ExpressionPart::Type(t) => {
                return ParamListOutcome::Err(format!(
                    "FN signature has a stray type `{}` outside a `<name>: <Type>` triple",
                    t.render(),
                ));
            }
            other => {
                return ParamListOutcome::Err(format!(
                    "FN signature part `{}` is not a Keyword, Identifier, or `<name>: <Type>` triple",
                    other.summarize(),
                ));
            }
        }
    }
    if let Some(msg) = first_err {
        return ParamListOutcome::Err(msg);
    }
    if !parks.is_empty() {
        return ParamListOutcome::Park(parks);
    }
    ParamListOutcome::Done(elements)
}

/// Dispatch-time placeholder extractor for FN. The signature slot at `parts[1]` is an
/// `Expression(signature_expr)` whose first `Keyword` is the function's name (the same
/// name used by `body` to register the function — see the `find_map(SignatureElement::
/// Keyword, ...)` call). Walks the signature parts inline rather than re-running the
/// full `parse_fn_param_list`; the body still does the full parse and surfaces any shape
/// errors. Returns `None` if the signature slot is missing or malformed (e.g. no Keyword
/// in the signature) — the body's `ShapeError` reports the real failure.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    let sig_part = expr.parts.get(1)?;
    let signature_expr = match sig_part {
        ExpressionPart::Expression(boxed) => boxed,
        _ => return None,
    };
    for part in &signature_expr.parts {
        if let ExpressionPart::Keyword(s) = part {
            return Some(s.clone());
        }
    }
    None
}
