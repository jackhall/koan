//! Signature parsing for the `FN` builtin.
//!
//! Two entry points:
//! - [`parse_fn_param_list`] — full structural parse at FN construction time. Returns
//!   [`ParamListOutcome`] so unresolved parameter types can route through a `Combine`.
//! - [`pre_run`] — dispatch-time placeholder extractor that announces the function's name
//!   before its body runs.

use crate::machine::model::{Argument, KObject, SignatureElement};
use crate::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator, Parseable};
use crate::machine::NodeId;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeParams};

/// Extract parameter names from an FN signature's `KExpression` shape without running
/// type elaboration. Must run before any outer-scope elaboration, otherwise the eager
/// path would surface `Unbound` against a parameter name. Returns names in declaration
/// order.
pub(super) fn collect_param_names_from_signature(signature: &KExpression<'_>) -> Vec<String> {
    let parts = &signature.parts;
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let param_name: Option<String> = match &parts[i] {
            ExpressionPart::Identifier(name) => Some(name.clone()),
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                Some(t.name.clone())
            }
            _ => None,
        };
        if let Some(name) = param_name {
            let next = parts.get(i + 1);
            let next_is_type_slot = matches!(
                next,
                Some(ExpressionPart::Type(_))
                    | Some(ExpressionPart::Expression(_))
                    | Some(ExpressionPart::Future(_))
            );
            if next_is_type_slot {
                names.push(name);
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    names
}

/// Result of one walk over an FN signature's part list.
pub(super) enum ParamListOutcome<'a> {
    Done(Vec<SignatureElement>),
    /// One or more parameter slots couldn't elaborate synchronously. The caller schedules
    /// a `Combine` over `park_producers` and any sub-Dispatches spawned from
    /// `sub_dispatches`; when every dep terminalizes, the closure splices each
    /// sub-Dispatch's `KObject::KTypeValue` result into the corresponding slot of
    /// `signature_expr.parts` (replacing the `Expression(_)` part with `Future(obj)`)
    /// and re-runs `parse_fn_param_list`.
    Pending {
        park_producers: Vec<NodeId>,
        /// Each entry is `(slot_idx_in_signature_parts, sub_expr_to_dispatch)`. The
        /// caller pairs each scheduled `NodeId` with its `slot_idx` so the Combine finish
        /// can splice results back into the right places.
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
    Err(String),
}

/// Convert the captured FN-parameter-list `KExpression` into a list of `SignatureElement`s.
///
/// Type-name resolution rides on the scheduler-aware [`elaborate_type_expr`], which
/// consults the captured scope's `placeholders` map alongside its `data` map and returns
/// `ElabResult::Park(producers)` for type-binding names that have dispatched but not
/// finalized. Parking producers and parens-wrapped sub-Dispatches accumulate across the
/// whole signature walk so the caller can register every blocker in one Combine.
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
        // A bare-leaf `Type` part (e.g. `Er` in `FN (LIFT Er: OrderedSig) -> ...`) parses
        // as `Type(TypeExpr { name, params: None })` per classify_atom, but in
        // parameter-name position semantically denotes a binder, not a type reference.
        let param_name: Option<String> = match &parts[i] {
            ExpressionPart::Identifier(name) => Some(name.clone()),
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                Some(t.name.clone())
            }
            _ => None,
        };
        match (param_name, &parts[i]) {
            (_, ExpressionPart::Keyword(s)) => {
                elements.push(SignatureElement::Keyword(s.clone()));
                i += 1;
            }
            (Some(name), _) => {
                let ty = parts.get(i + 1);
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
                        i += 2;
                    }
                    Some(ExpressionPart::Expression(boxed)) => {
                        // `slot_idx` is the part's position in `signature.parts` so the
                        // Combine finish can splice the result back into the right slot.
                        sub_dispatches.push((i + 1, (**boxed).clone()));
                        i += 2;
                    }
                    Some(ExpressionPart::Future(KObject::KTypeValue(kt))) => {
                        elements.push(SignatureElement::Argument(Argument {
                            name: name.clone(),
                            ktype: (*kt).clone(),
                        }));
                        i += 2;
                    }
                    Some(ExpressionPart::Future(other)) => {
                        return ParamListOutcome::Err(format!(
                            "FN signature parameter `{name}` type slot resolved to a non-type \
                             value `{}` (expected a type expression like `:Number` or `:(List Str)`)",
                            other.summarize(),
                        ));
                    }
                    _ => {
                        return ParamListOutcome::Err(format!(
                            "FN signature parameter `{name}` requires a `:<Type>` annotation \
                             (e.g. `{name} :Number`)",
                        ));
                    }
                }
            }
            (None, ExpressionPart::Type(t)) => {
                return ParamListOutcome::Err(format!(
                    "FN signature has a stray type `{}` outside a `<name> :<Type>` pair",
                    t.render(),
                ));
            }
            (None, other) => {
                return ParamListOutcome::Err(format!(
                    "FN signature part `{}` is not a Keyword, Identifier, or `<name> :<Type>` pair",
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
/// `Expression(signature_expr)` whose first `Keyword` is the function's registered name.
/// Returns `None` if the signature slot is missing or malformed — `body`'s full parse
/// surfaces the real `ShapeError`.
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
