//! Signature parsing for the `FN` builtin.

use crate::machine::model::Carried;
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
        let param_name: Option<String> = match &parts[i].value {
            ExpressionPart::Identifier(name) => Some(name.clone()),
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        };
        if let Some(name) = param_name {
            let next = parts.get(i + 1).map(|p| &p.value);
            let next_is_type_slot = matches!(
                next,
                Some(ExpressionPart::Type(_))
                    | Some(ExpressionPart::Expression(_))
                    | Some(ExpressionPart::SigiledTypeExpr(_))
                    | Some(ExpressionPart::RecordType(_))
                    | Some(ExpressionPart::Spliced { .. })
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
    /// One or more parameter slots couldn't elaborate synchronously. The caller schedules
    /// a `AwaitDeps` over `park_producers` and any sub-Dispatches; the closure splices each
    /// sub-Dispatch's `Carried::Type` result into the corresponding slot of
    /// `signature_expr.parts` (replacing `Expression(_)` with `Spliced(obj)`) and re-runs
    /// `parse_fn_param_list`.
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
pub(crate) fn parse_fn_param_list<'a>(
    signature: &KExpression<'a>,
    elaborator: &mut Elaborator<'_, 'a>,
    types: &TypeRegistry,
) -> ParamListOutcome<'a> {
    let parts = &signature.parts;
    let mut elements: Vec<SignatureElement> = Vec::with_capacity(parts.len());
    let mut parks: Vec<NodeId> = Vec::new();
    let mut sub_dispatches: Vec<(usize, KExpression<'a>)> = Vec::new();
    let mut first_err: Option<String> = None;
    let mut i = 0;
    while i < parts.len() {
        // A bare-leaf `Type` part (e.g. `er` in `FN (LIFT er: Ordered) -> ...`) in
        // parameter-name position denotes a binder, not a type reference.
        let param_name: Option<String> = match &parts[i].value {
            ExpressionPart::Identifier(name) => Some(name.clone()),
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        };
        match (param_name, &parts[i].value) {
            (_, ExpressionPart::Keyword(s)) => {
                elements.push(SignatureElement::Keyword(s.clone()));
                i += 1;
            }
            (Some(name), _) => {
                let ty = parts.get(i + 1).map(|p| &p.value);
                match ty {
                    Some(ExpressionPart::Type(t)) => {
                        match elaborate_type_identifier(elaborator, t, types) {
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
                    Some(ExpressionPart::Expression(boxed)) => {
                        sub_dispatches.push((i + 1, (**boxed).clone()));
                        i += 2;
                    }
                    Some(ExpressionPart::SigiledTypeExpr(boxed)) => {
                        // Wrap and sub-Dispatch so the dispatcher routes the inner
                        // expression through its standard classifier; the dep-finish
                        // splices the type-side carrier back as `Spliced(_)`.
                        let wrapped = KExpression::new(vec![Spanned::bare(
                            ExpressionPart::SigiledTypeExpr(boxed.clone()),
                        )]);
                        sub_dispatches.push((i + 1, wrapped));
                        i += 2;
                    }
                    Some(ExpressionPart::RecordType(boxed)) => {
                        // A `:{…}` record param type sub-Dispatches to a `KType::Record`
                        // carrier the dep-finish splices back as `Spliced(_)`.
                        let wrapped = KExpression::new(vec![Spanned::bare(
                            ExpressionPart::RecordType(boxed.clone()),
                        )]);
                        sub_dispatches.push((i + 1, wrapped));
                        i += 2;
                    }
                    Some(ExpressionPart::Spliced { cell }) => {
                        // The resolved type slot arrives as a carrier cell. A type is owned data, so
                        // it is read straight out of the envelope and cloned into the signature's
                        // own `Argument` — no adoption, no allocation.
                        let cloned = cell.open(|live| match live {
                            Carried::Type(kt) => Ok(kt.clone()),
                            other => Err(other.summarize(types)),
                        });
                        match cloned {
                            Ok(ktype) => {
                                elements.push(SignatureElement::Argument(Argument {
                                    name: name.clone(),
                                    ktype,
                                }));
                                i += 2;
                            }
                            Err(summary) => {
                                return ParamListOutcome::Err(format!(
                                    "FN signature parameter `{name}` type slot resolved to a \
                                     non-type value `{summary}` (expected a type expression like \
                                     `:Number` or `:(LIST OF Str)`)",
                                ));
                            }
                        }
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

/// Dispatch-time bucket-key extractor for FN. The key must match what a
/// future call would compute via `KExpression::untyped_key`: each Keyword maps to
/// `UntypedElement::Keyword`, and each `<name> :<Type>` pair collapses to one
/// `UntypedElement::Slot`.
///
/// Unknown shapes advance silently — the body's full parse surfaces `ShapeError` on
/// real malformations, so we err toward producing the bucket key for well-formed
/// signatures. An FN registers exactly one overload, so the returned vector holds one key.
/// Returns `None` only when the signature slot itself is missing.
pub(crate) fn binder_bucket(
    expr: &KExpression<'_>,
) -> Option<Vec<crate::machine::model::UntypedKey>> {
    use crate::machine::model::UntypedElement;
    let signature_expr = signature_expr_part(expr)?;
    let parts = &signature_expr.parts;
    let mut key = Vec::with_capacity(parts.len());
    let mut i = 0;
    while i < parts.len() {
        match &parts[i].value {
            ExpressionPart::Keyword(s) => {
                key.push(UntypedElement::Keyword(s.clone()));
                i += 1;
            }
            ExpressionPart::Identifier(_) => {
                let next_is_type_slot = parts.get(i + 1).is_some_and(|p| {
                    matches!(
                        p.value,
                        ExpressionPart::Type(_)
                            | ExpressionPart::Expression(_)
                            | ExpressionPart::SigiledTypeExpr(_)
                            | ExpressionPart::RecordType(_)
                            | ExpressionPart::Spliced { .. }
                    )
                });
                if next_is_type_slot {
                    key.push(UntypedElement::Slot);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ExpressionPart::Type(_) => {
                let next_is_type_slot = parts.get(i + 1).is_some_and(|p| {
                    matches!(
                        p.value,
                        ExpressionPart::Type(_)
                            | ExpressionPart::Expression(_)
                            | ExpressionPart::SigiledTypeExpr(_)
                            | ExpressionPart::RecordType(_)
                            | ExpressionPart::Spliced { .. }
                    )
                });
                if next_is_type_slot {
                    key.push(UntypedElement::Slot);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    Some(vec![key])
}

fn signature_expr_part<'a, 'b>(expr: &'b KExpression<'a>) -> Option<&'b KExpression<'a>> {
    let sig_part = expr.parts.get(1)?;
    match &sig_part.value {
        ExpressionPart::Expression(boxed) => Some(boxed.as_ref()),
        _ => None,
    }
}
