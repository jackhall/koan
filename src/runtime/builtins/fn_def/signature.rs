//! Signature parsing for the `FN` builtin.
//!
//! Two entry points:
//! - [`parse_fn_param_list`] — the full structural parse used by [`super::body`] at FN
//!   construction time. Returns a [`ParamListOutcome`] so the caller can route through a
//!   `Combine` when one or more parameter-type names resolve to a pending placeholder *or*
//!   when one or more parameter slots use a parens-wrapped type expression that needs
//!   sub-Dispatch.
//! - [`pre_run`] — the dispatch-time placeholder extractor used by `register` to
//!   announce the function's name before its body runs.

use crate::runtime::model::{Argument, KObject, SignatureElement};
use crate::runtime::model::types::{elaborate_type_expr, ElabResult, Elaborator, Parseable};
use crate::runtime::machine::NodeId;
use crate::ast::{ExpressionPart, KExpression};

/// Result of one walk over an FN signature's part list.
pub(super) enum ParamListOutcome<'a> {
    /// Every parameter type elaborated against the captured scope; the resulting
    /// `SignatureElement`s are ready to bind.
    Done(Vec<SignatureElement>),
    /// One or more parameter slots couldn't elaborate synchronously. The caller schedules
    /// a `Combine` over [`Self::park_producers`] and any sub-Dispatches it spawns from
    /// [`Self::sub_dispatches`]; when every dep terminalizes, the closure splices each
    /// sub-Dispatch's `KObject::KTypeValue` result into the corresponding slot of
    /// `signature_expr.parts` (replacing the `Expression(_)` part with `Future(obj)`)
    /// and re-runs `parse_fn_param_list`.
    Pending {
        /// Producers (placeholders) the walk parked on; unchanged from the previous
        /// `Park(_)`-only contract.
        park_producers: Vec<NodeId>,
        /// Parens-wrapped type slots needing sub-Dispatch. Each entry is
        /// `(slot_idx_in_signature_parts, sub_expr_to_dispatch)`. The caller pairs each
        /// scheduled `NodeId` with its `slot_idx` so the Combine finish can splice
        /// results back into the right places.
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
    /// A structural / unbound / cycle error surfaced during elaboration. The caller wraps
    /// in `ShapeError`.
    Err(String),
}

/// Convert the captured FN-parameter-list `KExpression` into a list of `SignatureElement`s.
/// Walks the parts left-to-right, consuming bare `Keyword` parts as fixed tokens and
/// `Identifier(name) Keyword(":") <type-slot>` triples as typed `Argument` slots, where
/// `<type-slot>` is one of:
///
/// - `Type(t)` — bare type token (the original surface form).
/// - `Expression(e)` — parens-wrapped type expression like `(LIST_OF Number)`. The walk
///   records its `(slot_idx, e)` for the caller to schedule as a sub-Dispatch; the slot
///   is left unfilled until the Combine wakes and a re-walk sees the spliced
///   `Future(KTypeValue(_))`.
/// - `Future(KObject::KTypeValue(kt))` — already-resolved type value spliced in by the
///   Combine finish. Lifted directly into the slot's `KType`.
///
/// Stray `:`, stray `Type`, missing `: Type` annotations, and other malformed shapes
/// surface as [`ParamListOutcome::Err`].
///
/// Type-name resolution rides on the scheduler-aware [`elaborate_type_expr`]: it consults
/// the captured scope's `placeholders` map alongside its `data` map, returning
/// `ElabResult::Park(producers)` for type-binding names that have dispatched but not
/// finalized. Multiple parking producers and parens-wrapped sub-Dispatches accumulate
/// across the whole signature walk so the caller can register every blocker in one
/// Combine.
pub(super) fn parse_fn_param_list<'a>(
    signature: &KExpression<'a>,
    elaborator: &mut Elaborator<'_, '_>,
) -> ParamListOutcome<'a> {
    let parts = &signature.parts;
    let mut elements: Vec<SignatureElement> = Vec::with_capacity(parts.len());
    let mut parks: Vec<NodeId> = Vec::new();
    let mut sub_dispatches: Vec<(usize, KExpression<'a>)> = Vec::new();
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
                let is_colon = matches!(colon, Some(ExpressionPart::Keyword(c)) if c == ":");
                if !is_colon {
                    return ParamListOutcome::Err(format!(
                        "FN signature parameter `{name}` requires a `: Type` annotation \
                         (e.g. `{name}: Number`)",
                    ));
                }
                match ty {
                    Some(ExpressionPart::Type(t)) => {
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
                    Some(ExpressionPart::Expression(boxed)) => {
                        // Parens-wrapped type expression (`xs: (LIST_OF Number)`). Schedule
                        // its sub-Dispatch via the caller; the result splices back as a
                        // `Future(KTypeValue(_))` on re-walk. Record `(slot_idx, sub_expr)`
                        // — `slot_idx` is the position of this `Expression` part within
                        // `signature.parts` so the splice goes to the right place.
                        sub_dispatches.push((i + 2, (**boxed).clone()));
                        i += 3;
                    }
                    Some(ExpressionPart::Future(KObject::KTypeValue(kt))) => {
                        // Spliced result from a prior sub-Dispatch (Combine wake re-walk).
                        // Lift the carried `KType` directly into the slot.
                        elements.push(SignatureElement::Argument(Argument {
                            name: name.clone(),
                            ktype: (*kt).clone(),
                        }));
                        i += 3;
                    }
                    Some(ExpressionPart::Future(other)) => {
                        return ParamListOutcome::Err(format!(
                            "FN signature parameter `{name}` type slot resolved to a non-type \
                             value `{}` (expected a type expression like `Number` or `List<Str>`)",
                            other.summarize(),
                        ));
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
    if !parks.is_empty() || !sub_dispatches.is_empty() {
        return ParamListOutcome::Pending {
            park_producers: parks,
            sub_dispatches,
        };
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
