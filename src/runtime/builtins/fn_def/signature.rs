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

use crate::runtime::machine::model::{Argument, KObject, SignatureElement};
use crate::runtime::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator, Parseable};
use crate::runtime::machine::NodeId;
use crate::ast::{ExpressionPart, KExpression, TypeParams};

/// Extract parameter names from an FN signature's `KExpression` shape without running
/// type elaboration. Used by Stage B's return-type scan (see
/// [`super::body`]) to decide between `ReturnType::Resolved` and
/// `ReturnType::Deferred` before any outer-scope elaboration runs — the scan must
/// happen at FN-def time, before the eager-elaborate path would surface an `Unbound`
/// against a parameter name.
///
/// Walks the same `(Identifier|Type) ":" <type-slot>` triple shape the full parser
/// recognizes, but skips the type-slot validation (anything that looks like a typed
/// param contributes the bare name). Returns names in declaration order.
pub(super) fn collect_param_names_from_signature(signature: &KExpression<'_>) -> Vec<String> {
    let parts = &signature.parts;
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        // Recognize a parameter-name slot: either a lowercase `Identifier` (`xs`)
        // or a Type-classified bare-leaf token (`Er`, `Elem` — Stage A's surface
        // form). The `<name>: <type>` shape requires the next part to be a `:`.
        let param_name: Option<String> = match &parts[i] {
            ExpressionPart::Identifier(name) => Some(name.clone()),
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                Some(t.name.clone())
            }
            _ => None,
        };
        if let Some(name) = param_name {
            let colon = parts.get(i + 1);
            let is_colon = matches!(colon, Some(ExpressionPart::Keyword(c)) if c == ":");
            if is_colon {
                names.push(name);
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    names
}

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
        // Recognize the parameter-name slot up front: either a lowercase `Identifier`
        // (`xs`, `elem`) or a Type-classified bare-leaf token (`Er`, `Elem`). The
        // Type-classified case is what makes `FN (LIFT Er: OrderedSig) -> ...` work —
        // `Er` parses as `Type(TypeExpr { name: "Er", params: None })` per the
        // tokenizer's classify_atom rules, but in *parameter-name position* it
        // semantically denotes a binder name, not a type reference. Module-system
        // functor-params Stage A: dual-write of the per-call value's type-language
        // identity in `KFunction::invoke` makes this binder name accessible to the
        // FN body's type-position references, which is the whole point of admitting
        // it here.
        let param_name: Option<String> = match &parts[i] {
            ExpressionPart::Identifier(name) => Some(name.clone()),
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                Some(t.name.clone())
            }
            _ => None,
        };
        match (param_name, &parts[i]) {
            (_, ExpressionPart::Keyword(s)) if s == ":" => {
                return ParamListOutcome::Err(
                    "FN signature has a stray `:` outside a `<name>: <Type>` triple".to_string(),
                );
            }
            (_, ExpressionPart::Keyword(s)) => {
                elements.push(SignatureElement::Keyword(s.clone()));
                i += 1;
            }
            (Some(name), _) => {
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
            (None, ExpressionPart::Type(t)) => {
                // Type-classified token with parameters (`Foo<Bar>`) outside the
                // `<name>: <Type>` triple is a stray type — the bare-leaf in-position
                // case is already handled above.
                return ParamListOutcome::Err(format!(
                    "FN signature has a stray type `{}` outside a `<name>: <Type>` triple",
                    t.render(),
                ));
            }
            (None, other) => {
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
