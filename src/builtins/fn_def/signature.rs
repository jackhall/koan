//! Signature parsing for the `FN` builtin.

use crate::machine::ProducerId;
use crate::machine::model::KType;
use crate::machine::model::{Argument, SignatureElement};
use crate::machine::model::{BinderSymbol, RunRegistries, Symbol};
use crate::machine::model::{Elaborator, TypeResolution, elaborate_type_identifier};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{SignaturePosition, SignatureScan};
use crate::source::Spanned;
use crate::witnessed::{BumpAllocator, BumpVec};

/// Must run before any outer-scope elaboration: the eager path would otherwise surface
/// `Unbound` against a parameter name.
///
/// Only an annotated position names a parameter; the stride itself is
/// [`SignatureScan`]'s, shared with [`parse_fn_param_list`].
pub(crate) fn collect_param_names_from_signature<'s>(
    signature: &KExpression<'_>,
    scratch: BumpAllocator<'s>,
) -> BumpVec<'s, Symbol> {
    let parts = signature.parts;
    // Read by the return-type classifier and dropped inside this step, so the scratch hosts it.
    // A name takes a part and its type slot takes another, so the parts count is the upper bound.
    let mut names: BumpVec<'s, Symbol> = BumpVec::with_capacity_in(parts.len(), scratch);
    for position in SignatureScan::new(parts) {
        // The scan compares against reference leaves, which probe by bare symbol bits, so a
        // parameter's name rides as the symbol its own token minted.
        if let SignaturePosition::Annotated { name, .. } = position {
            names.push(name.symbol());
        }
    }
    names
}

/// The diagnostic a binder position with no `:<Type>` annotation reports.
fn missing_annotation(symbol: BinderSymbol, registries: &RunRegistries) -> String {
    let name = crate::machine::model::render_label(symbol.symbol(), registries);
    format!(
        "FN signature parameter `{name}` requires a `:<Type>` annotation (e.g. `{name} :Number`)",
    )
}

pub(crate) enum ParamListOutcome<'a> {
    Done(BumpVec<'a, SignatureElement>),
    /// One or more parameter slots couldn't elaborate synchronously. The caller schedules an
    /// `AwaitDeps` over `awaited_producers` and any sub-Dispatches, then re-runs
    /// `parse_fn_param_list` over the same (unmodified) `signature` with the resolved
    /// sub-Dispatch carriers fed back through its `resolved` parameter — `signature` is raw AST
    /// throughout and never carries a scheduler-written slot.
    Pending {
        awaited_producers: Vec<ProducerId>,
        /// `(slot_idx_in_signature_parts, sub_expr_to_dispatch)`.
        sub_dispatches: BumpVec<'a, (usize, KExpression<'a>)>,
    },
    Err(String),
}

/// Type-name resolution rides on [`elaborate_type_identifier`], which returns
/// `TypeResolution::Park(sources)` for type-binding names that have dispatched but not
/// finalized. Park sources and sub-Dispatches accumulate across the whole signature
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
    registries: &RunRegistries,
    resolved: Option<&[(usize, KType)]>,
    scratch: BumpAllocator<'a>,
) -> ParamListOutcome<'a> {
    let parts = signature.parts;
    // Keyword tokens keep riding as `&'a str`; the mint door re-homes those at the function's own
    // region. The run itself is staging: the mint copies it into the callable's region and it dies
    // with this step, so it takes the step scratch, one push per part making the reservation exact.
    let mut elements: BumpVec<'a, SignatureElement> =
        BumpVec::with_capacity_in(parts.len(), scratch);
    // `awaited` is the one buffer here that stays on the heap. It grows by `extend` over a
    // `TypeResolution::Park`'s producer list, whose length the resolver sets rather than this
    // loop, so no bound is available to reserve against — and a bump fill that outgrows its
    // reservation abandons the old buffer as dead region bytes.
    let mut awaited: Vec<ProducerId> = Vec::new();
    // Its peer parks at most one sub-dispatch per part and is read back inside this same step, by
    // the `defer` that schedules them, so the scratch hosts it against an exact reservation.
    let mut sub_dispatches: BumpVec<'a, (usize, KExpression<'a>)> =
        BumpVec::with_capacity_in(parts.len(), scratch);
    let mut first_err: Option<String> = None;
    for position in SignatureScan::new(parts) {
        match position {
            SignaturePosition::Keyword(keyword) => {
                elements.push(SignatureElement::Keyword(keyword));
            }
            SignaturePosition::Annotated {
                name: symbol,
                annotation,
            } => {
                let feed = resolved.and_then(|r| {
                    r.iter()
                        .find_map(|(idx, ktype)| (*idx == annotation).then_some(*ktype))
                });
                match (parts[annotation].value, feed) {
                    (ExpressionPart::Type(t), _) => {
                        match elaborate_type_identifier(elaborator, t, registries) {
                            TypeResolution::Done(kt) => {
                                elements.push(SignatureElement::Argument(Argument {
                                    name: symbol,
                                    ktype: kt,
                                }));
                            }
                            TypeResolution::Park(producers) => {
                                awaited.extend(producers);
                            }
                            TypeResolution::Unbound(missing) if first_err.is_none() => {
                                first_err = Some(format!(
                                    "{} in FN signature for parameter `{}`",
                                    crate::machine::model::unknown_type_name(missing, registries),
                                    crate::machine::model::display_label(
                                        symbol.symbol(),
                                        registries
                                    ),
                                ));
                            }
                            TypeResolution::Unbound(_) => {}
                        }
                    }
                    (
                        ExpressionPart::Expression(_)
                        | ExpressionPart::SigiledTypeExpr(_)
                        | ExpressionPart::RecordType(_),
                        Some(ktype),
                    ) => {
                        // The dep-finish re-walk: this slot's sub-Dispatch already resolved, and the
                        // finish rejected a non-type terminal before feeding it here. The type is an
                        // interned handle, fed back positionally rather than spliced into the
                        // expression.
                        elements.push(SignatureElement::Argument(Argument {
                            name: symbol,
                            ktype,
                        }));
                    }
                    (ExpressionPart::Expression(inner), None) => {
                        sub_dispatches.push((annotation, *inner));
                    }
                    (ExpressionPart::SigiledTypeExpr(inner), None) => {
                        // Wrap and sub-Dispatch so the dispatcher routes the inner expression
                        // through its standard classifier.
                        let brand = elaborator.scope.brand();
                        let wrapped = KExpression::new(
                            brand,
                            &[Spanned::bare(ExpressionPart::SigiledTypeExpr(inner))],
                        );
                        sub_dispatches.push((annotation, wrapped));
                    }
                    (ExpressionPart::RecordType(inner), None) => {
                        // A `:{…}` record param type sub-Dispatches to a record `KType` carrier.
                        let brand = elaborator.scope.brand();
                        let wrapped = KExpression::new(
                            brand,
                            &[Spanned::bare(ExpressionPart::RecordType(inner))],
                        );
                        sub_dispatches.push((annotation, wrapped));
                    }
                    // `SignatureScan` only pairs a name with one of the four shapes above, so this
                    // arm is the exhaustiveness tail rather than a reachable input.
                    _ => {
                        return ParamListOutcome::Err(missing_annotation(symbol, registries));
                    }
                }
            }
            SignaturePosition::Bare(symbol) => {
                return ParamListOutcome::Err(missing_annotation(symbol, registries));
            }
            SignaturePosition::Foreign(at) => {
                return ParamListOutcome::Err(format!(
                    "FN signature part `{}` is not a Keyword, Identifier, or `<name> :<Type>` pair",
                    parts[at].value.summary(&registries.labels),
                ));
            }
        }
    }
    if let Some(msg) = first_err {
        return ParamListOutcome::Err(msg);
    }
    if !awaited.is_empty() || !sub_dispatches.is_empty() {
        return ParamListOutcome::Pending {
            awaited_producers: awaited,
            sub_dispatches,
        };
    }
    ParamListOutcome::Done(elements)
}
