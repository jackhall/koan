//! Signature parsing for the `FN` builtin.

use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{elaborate_type_identifier, Elaborator, TypeResolution};
use crate::machine::model::{Argument, SignatureElement};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::NodeId;
use crate::source::Spanned;

/// Must run before any outer-scope elaboration: the eager path would otherwise surface
/// `Unbound` against a parameter name.
pub(crate) fn collect_param_names_from_signature(signature: &KExpression<'_>) -> Vec<String> {
    let parts = &signature.parts;
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let param_name: Option<String> = match parts[i].value {
            ExpressionPart::Identifier(name) => Some(name.to_string()),
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        };
        if let Some(name) = param_name {
            let next = parts.get(i + 1).map(|p| p.value);
            let next_is_type_slot = matches!(
                next,
                Some(ExpressionPart::Type(_))
                    | Some(ExpressionPart::Expression(_))
                    | Some(ExpressionPart::SigiledTypeExpr(_))
                    | Some(ExpressionPart::RecordType(_))
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

pub(crate) enum ParamListOutcome<'a> {
    Done(Vec<SignatureElement>),
    /// One or more parameter slots couldn't elaborate synchronously. The caller schedules an
    /// `AwaitDeps` over `park_producers` and any sub-Dispatches, then re-runs
    /// `parse_fn_param_list` over the same (unmodified) `signature` with the resolved
    /// sub-Dispatch carriers fed back through its `resolved` parameter — `signature` is raw AST
    /// throughout and never carries a scheduler-written slot.
    Pending {
        park_producers: Vec<NodeId>,
        /// `(slot_idx_in_signature_parts, sub_expr_to_dispatch)`.
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
    Err(String),
}

/// Type-name resolution rides on [`elaborate_type_identifier`], which returns
/// `TypeResolution::Park(producers)` for type-binding names that have dispatched but not
/// finalized. Parking producers and sub-Dispatches accumulate across the whole signature
/// walk so the caller can register every blocker in one dep-finish.
///
/// `resolved` is `None` on the first walk (every `Expression` / `SigiledTypeExpr` /
/// `RecordType` slot schedules a sub-Dispatch, recorded by part-index in
/// `ParamListOutcome::Pending::sub_dispatches`) and `Some` on the dep-finish re-walk: the walk
/// re-descends the same `signature`, and each such slot looks its resolved type up by that
/// same part-index instead of sub-dispatching again. The feed carries interned `KType` handles,
/// not carriers — the finish extracts them (and rejects a non-type terminal) while it holds the
/// dep envelopes open, so the walk borrows no producer region.
pub(crate) fn parse_fn_param_list<'a>(
    signature: &KExpression<'a>,
    elaborator: &mut Elaborator<'_, 'a>,
    types: &TypeRegistry,
    resolved: Option<&[(usize, KType)]>,
) -> ParamListOutcome<'a> {
    let parts = signature.parts;
    let mut elements: Vec<SignatureElement> = Vec::with_capacity(parts.len());
    let mut parks: Vec<NodeId> = Vec::new();
    let mut sub_dispatches: Vec<(usize, KExpression<'a>)> = Vec::new();
    let mut first_err: Option<String> = None;
    let mut i = 0;
    while i < parts.len() {
        // A bare-leaf `Type` part (e.g. `er` in `FN (LIFT er: Ordered) -> ...`) in
        // parameter-name position denotes a binder, not a type reference.
        let param_name: Option<String> = match parts[i].value {
            ExpressionPart::Identifier(name) => Some(name.to_string()),
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        };
        match (param_name, parts[i].value) {
            (_, ExpressionPart::Keyword(s)) => {
                elements.push(SignatureElement::Keyword(s.to_string()));
                i += 1;
            }
            (Some(name), _) => {
                let slot_idx = i + 1;
                let ty = parts.get(slot_idx).map(|p| p.value);
                let feed = resolved.and_then(|r| {
                    r.iter()
                        .find_map(|(idx, ktype)| (*idx == slot_idx).then_some(*ktype))
                });
                match (ty, feed) {
                    (Some(ExpressionPart::Type(t)), _) => {
                        match elaborate_type_identifier(elaborator, &t, types) {
                            TypeResolution::Done(kt) => {
                                elements.push(SignatureElement::Argument(Argument {
                                    name: name.clone(),
                                    ktype: kt,
                                }));
                            }
                            TypeResolution::Park(producers) => {
                                parks.extend(producers);
                            }
                            TypeResolution::Unbound(msg) if first_err.is_none() => {
                                first_err =
                                    Some(format!("{msg} in FN signature for parameter `{name}`"));
                            }
                            TypeResolution::Unbound(_) => {}
                        }
                        i += 2;
                    }
                    (
                        Some(
                            ExpressionPart::Expression(_)
                            | ExpressionPart::SigiledTypeExpr(_)
                            | ExpressionPart::RecordType(_),
                        ),
                        Some(ktype),
                    ) => {
                        // The dep-finish re-walk: this slot's sub-Dispatch already resolved, and the
                        // finish rejected a non-type terminal before feeding it here. The type is an
                        // interned handle, fed back positionally rather than spliced into the
                        // expression.
                        elements.push(SignatureElement::Argument(Argument {
                            name: name.clone(),
                            ktype,
                        }));
                        i += 2;
                    }
                    (Some(ExpressionPart::Expression(inner)), None) => {
                        sub_dispatches.push((slot_idx, *inner));
                        i += 2;
                    }
                    (Some(ExpressionPart::SigiledTypeExpr(inner)), None) => {
                        // Wrap and sub-Dispatch so the dispatcher routes the inner expression
                        // through its standard classifier.
                        let brand = elaborator.scope.brand();
                        let wrapped = KExpression::new(
                            brand,
                            vec![Spanned::bare(ExpressionPart::SigiledTypeExpr(inner))],
                        );
                        sub_dispatches.push((slot_idx, wrapped));
                        i += 2;
                    }
                    (Some(ExpressionPart::RecordType(inner)), None) => {
                        // A `:{…}` record param type sub-Dispatches to a `KType::Record` carrier.
                        let brand = elaborator.scope.brand();
                        let wrapped = KExpression::new(
                            brand,
                            vec![Spanned::bare(ExpressionPart::RecordType(inner))],
                        );
                        sub_dispatches.push((slot_idx, wrapped));
                        i += 2;
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
