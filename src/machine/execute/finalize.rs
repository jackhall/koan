use std::rc::Rc;

use crate::machine::core::{FoldingBrand, FrameStorage, KoanStorageProfile};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{Carried, KType, TypeNode, TypeRegistry};
use crate::machine::{DeliveredCarried, KError, KErrorKind};

use super::harness::Host;
use super::obligation::ReturnObligation;

/// How a finished value disposes against its declared return, decided by a single read pass over the
/// delivered carrier before anything is allocated.
enum Disposition {
    /// The value satisfies the contract and keeps its runtime type — a type-channel pass, or a
    /// declared *union* return (union elimination dispatches on the value's own runtime type, so it
    /// is never re-stamped). The delivery envelope travels on verbatim.
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
    /// framed producer with no obligation) the envelope travels on **as-is** — the delivery walk
    /// that consumes it is what moves the value, so at this boundary it still resides where it was
    /// born. A declared-return
    /// check runs one read pass over the delivered carrier; a satisfying non-union object re-stamps
    /// to the declared type **in place, in the producer's own region** ([`Delivered::restamp_in_place`](crate::witnessed::Delivered::restamp_in_place)) —
    /// no bytes move, residence is unchanged — while a union return and a type value pass through
    /// un-restamped and a mismatch raises.
    ///
    /// `home` is the owner of the region the value lives in — the slot's anchor owner, the same one
    /// the envelope was sealed under. It is passed in rather than read off the envelope: the
    /// envelope's members are one flat antichain in which home is an ordinary member, and the
    /// re-stamp's destination must be exactly the region the value already resides in.
    ///
    /// The terminal leaves as the envelope it arrived in (or the re-stamp's product envelope) — the
    /// [`DeliveredTerminal`](crate::scheduler::DeliveredTerminal) currency
    /// [`Scheduler::finalize`](crate::scheduler::Scheduler::finalize) consumes whole. Nothing here
    /// splits the carrier from its coverage: the walk derives each destination's adopt from this
    /// very envelope inside the scheduler, so no call site can pair a terminal with a coverage
    /// that is not its own.
    fn finalize_terminal(
        &self,
        envelope: DeliveredCarried,
        home: &Rc<FrameStorage>,
        contract: Option<&ReturnObligation>,
    ) -> Result<DeliveredCarried, KError>;
}

impl NodeFinalize for Host<'_> {
    fn finalize_terminal(
        &self,
        envelope: DeliveredCarried,
        home: &Rc<FrameStorage>,
        contract: Option<&ReturnObligation>,
    ) -> Result<DeliveredCarried, KError> {
        // The terminal's owned member set is invariant across finalize: pass-through hands the
        // envelope on verbatim, and restamp re-stamps *in place in the producer's own region*, so
        // the product envelope's members are identical to the input's. Either way the coverage never
        // leaves the envelope, so the reach the scheduler's delivery walk adopts against is the
        // reach of the value it stores.
        //
        // No per-call return obligation (frameless / run producer, or a framed producer with no
        // obligation) or nothing declared: the envelope passes through untouched — the walk decides
        // residence, so the Done boundary makes no memory decision.
        let Some(obligation) = contract else {
            return Ok(envelope);
        };
        let Some((declared, per_call)) = obligation.declared() else {
            return Ok(envelope);
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
            Disposition::PassThrough => Ok(envelope),
            // Re-stamp in place: re-tag the top node to the declared type and re-anchor it into the
            // producer's own region, sharing the substrate borrow verbatim. Residence is unchanged —
            // the re-mint's host is the same region — so the product envelope covers exactly what
            // the input did, and it is the product that travels on.
            Disposition::Restamp => Ok(envelope
                .restamp_in_place::<CarriedFamily, KoanStorageProfile>(
                    home,
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
/// producer's, re-stamped only later when the re-emitted `Forward` verdict finalizes). It inspects the value and its type and
/// adopts nothing, so it takes the value live at its caller's read brand rather than a carrier of
/// its own. Returns the labelled mismatch or `Ok(())`.
pub(in crate::machine::execute) fn check_spliced_return(
    obligation: &ReturnObligation,
    carried: Carried<'_>,
    types: &TypeRegistry,
) -> Result<(), KError> {
    let Some((declared, per_call)) = obligation.declared() else {
        return Ok(());
    };
    let label = obligation.label();
    let matched = match carried {
        Carried::Object(object) => declared.matches_value(object, types),
        Carried::Type(t) => declared.matches_type(t, types),
        // Every delivered result is resolved; an unlowered name satisfies no contract.
        Carried::UnresolvedType(_) => false,
    };
    if matched {
        return Ok(());
    }
    let got = match carried {
        Carried::Object(object) => object.ktype().name(types),
        Carried::Type(t) => t.name(types),
        Carried::UnresolvedType(ti) => ti.render(),
    };
    Err(return_type_mismatch(declared, per_call, label, got, types))
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
