use crate::machine::core::{FoldingBrand, KoanStorageProfile};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{Carried, KType, TypeNode, TypeRegistry};
use crate::machine::{CarrierWitness, DeliveredCarried, KError, KErrorKind};
use crate::witnessed::Witnessed;

use super::obligation::ReturnObligation;
use super::runtime::KoanRuntime;

/// How a finished value disposes against its declared return, decided by a single read pass over the
/// delivered carrier before anything is allocated.
enum Disposition {
    /// The value satisfies the contract and keeps its runtime type — a type-channel pass, or a
    /// declared *union* return (union elimination dispatches on the value's own runtime type, so it
    /// is never re-stamped). Recovers as-is through the seal→unseal round-trip.
    PassThrough,
    /// A non-union object that satisfies the contract and is re-stamped to the declared type, in
    /// place in its producer region.
    Restamp,
    /// The value does not satisfy the contract; carries the produced type's name for the diagnostic.
    Mismatch(String),
}

/// Seal a finished node's **value** terminal against its declared return contract, returning the
/// slot's final terminal. This hook receives the value already sealed into a delivery envelope
/// (pinned by the slot's anchor region owner) plus the step's [`ReturnObligation`], and reads the
/// obligation's precomputed declared return. The scheduler decides *when* (the Done boundary) and
/// hands over the sealed envelope; this hook owns the declared-return check and re-stamp *how*, so
/// the generic scheduler ([`crate::scheduler`]) names no Koan type. Errors carry no value and
/// finalize bare through [`finalize_error`], which never reaches this hook.
///
/// Peer of [`copy_carried`](super::lift::copy_carried): both are Done-boundary workload hooks.
pub(in crate::machine::execute) trait NodeFinalize {
    /// Seal the slot's value terminal against its declared return. With no declared return (or a
    /// framed producer with no obligation) the envelope's carrier recovers **as-is** through the
    /// seal→unseal round-trip — the scheduler's retention hold keeps the producer frame alive until
    /// every destination pulls, so the value keeps residing where it was born. A declared-return
    /// check runs one read pass over the delivered carrier; a satisfying non-union object re-stamps
    /// to the declared type **in place, in the producer's own region** ([`Delivered::restamp_in_place`](crate::witnessed::Delivered::restamp_in_place)) —
    /// no bytes move, residence is unchanged — while a union return and a type value pass through
    /// un-restamped and a mismatch raises.
    fn finalize_terminal(
        &self,
        envelope: DeliveredCarried,
        contract: Option<&ReturnObligation>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError>;
}

impl NodeFinalize for KoanRuntime<'_> {
    fn finalize_terminal(
        &self,
        envelope: DeliveredCarried,
        contract: Option<&ReturnObligation>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        // No per-call return obligation (frameless / run producer, or a framed producer with no
        // obligation) or nothing declared: recover the sealed carrier as-is via the seal→unseal
        // round-trip — retention owns the frame's lifetime, so the Done boundary makes no memory
        // decision.
        let Some(obligation) = contract else {
            return Ok(envelope.into_cell().unseal());
        };
        let Some((declared, per_call)) = obligation.declared() else {
            return Ok(envelope.into_cell().unseal());
        };
        let types = self.ambient.type_registry();
        // One read pass classifies the delivered carrier against the declared return under the
        // envelope's own host pin — no relocation, nothing allocated. An object checks by value; a
        // type checks by type; an unlowered name satisfies no contract.
        let disposition = envelope.open(|carried| match carried {
            Carried::Object(object) => {
                if !declared.matches_value(object, types) {
                    Disposition::Mismatch(object.ktype().name(types))
                } else if object.embeds_substrate()
                    && !matches!(types.node(declared), TypeNode::Union { .. })
                {
                    // Only a substrate carrier (`Record` / `List` / `Dict` / `Tagged` / `Wrapped`)
                    // carries a re-stampable type tag; a declared *union* return keeps its runtime
                    // type for union-elimination dispatch. Every other value satisfies the contract
                    // with its runtime type unchanged.
                    Disposition::Restamp
                } else {
                    Disposition::PassThrough
                }
            }
            Carried::Type(t) => {
                if declared.matches_type(t, types) {
                    Disposition::PassThrough
                } else {
                    Disposition::Mismatch(t.name(types))
                }
            }
            Carried::UnresolvedType(ti) => Disposition::Mismatch(ti.render()),
        });
        match disposition {
            Disposition::Mismatch(got) => Err(return_type_mismatch(
                declared,
                per_call,
                obligation.label(),
                got,
                types,
            )),
            Disposition::PassThrough => Ok(envelope.into_cell().unseal()),
            // Re-stamp in place: re-tag the top node to the declared type and re-anchor it into the
            // producer's own region, sharing the substrate borrow verbatim. Residence is unchanged,
            // so the re-sealed carrier's witness is identical to the delivered one.
            Disposition::Restamp => Ok(envelope
                .restamp_in_place::<CarriedFamily, KoanStorageProfile>(
                    |value, _handle, placement| {
                        let region = FoldingBrand::in_fold_closure(placement);
                        Carried::Object(region.alloc_object_folded(
                            value.object().deep_clone().stamp_type(declared, types),
                        ))
                    },
                )),
        }
    }
}

/// Label a `Done`-step **error** with its return contract's trace frame and return it for a bare
/// finalize. An error carries no value, so it needs no witness and no declared-return check (that
/// lives in [`finalize_terminal`](NodeFinalize::finalize_terminal), which errors never reach). A
/// `None` contract (the caller's `frame`-gate already folded in the dying-ness condition) carries no
/// per-call return obligation, so the error passes through unlabelled.
pub(in crate::machine::execute) fn finalize_error(
    error: KError,
    contract: Option<&ReturnObligation>,
) -> KError {
    match contract {
        Some(obligation) => {
            let label = obligation.label();
            error.with_frame(crate::machine::TraceFrame::bare(
                label.to_string(),
                label.to_string(),
            ))
        }
        None => error,
    }
}

/// Discharge a tail-spliced slot's residual declared-return obligation against the spliced producer's
/// delivered value — the checker micro-step's check, WITHOUT re-stamping (the value stays the
/// producer's, re-stamped only later when the re-emitted `Forward` finalizes through
/// [`NodeStep::ForwardReady`](super::nodes::NodeStep)). Reads the obligation's precomputed declared
/// return and matches by channel under the delivery envelope's own host pin. Returns the labelled
/// mismatch or `Ok(())`.
pub(in crate::machine::execute) fn check_spliced_return(
    obligation: &ReturnObligation,
    delivered: &DeliveredCarried,
    types: &TypeRegistry,
) -> Result<(), KError> {
    let Some((declared, per_call)) = obligation.declared() else {
        return Ok(());
    };
    let label = obligation.label();
    let mismatch = delivered.open(|carried| {
        let matched = match carried {
            Carried::Object(object) => declared.matches_value(object, types),
            Carried::Type(t) => declared.matches_type(t, types),
            // Every delivered result is resolved; an unlowered name satisfies no contract.
            Carried::UnresolvedType(_) => false,
        };
        if matched {
            return None;
        }
        let got = match carried {
            Carried::Object(object) => object.ktype().name(types),
            Carried::Type(t) => t.name(types),
            Carried::UnresolvedType(ti) => ti.render(),
        };
        Some(return_type_mismatch(declared, per_call, label, got, types))
    });
    match mismatch {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;

/// The labelled `TypeMismatch` a failed declared-return check raises. `expected` names the declared
/// type (tagged "per-call return type" for a `PerCall`); `got` names the produced carrier.
fn return_type_mismatch(
    declared: KType,
    per_call: bool,
    label: &str,
    got: String,
    types: &TypeRegistry,
) -> KError {
    let expected = if per_call {
        format!("{} (per-call return type)", declared.name(types))
    } else {
        declared.name(types)
    };
    KError::new(KErrorKind::TypeMismatch {
        arg: "<return>".to_string(),
        expected,
        got,
    })
    .with_frame(crate::machine::TraceFrame::bare(
        label.to_string(),
        label.to_string(),
    ))
}
