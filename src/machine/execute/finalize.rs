use std::rc::Rc;

use crate::machine::core::{FoldingBrand, FrameStorage, KoanStorageProfile};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{Carried, KType, TypeNode};
use crate::machine::{DeliveredCarried, KError, KErrorKind};

use super::harness::Host;
use super::obligation::ReturnObligation;
use crate::machine::model::RunRegistries;

/// How a finished value disposes against its declared return, decided by a single read pass over the
/// delivered carrier before anything is allocated.
enum Disposition {
    /// The value keeps its runtime type. A declared *union* return lands here too: union
    /// elimination dispatches on the value's own runtime type, so it is never re-stamped.
    PassThrough,
    Restamp,
    Mismatch(String),
}

/// The Done-boundary hook that checks a finished node's **value** terminal against its declared
/// return contract. Owning the check here is what keeps the generic scheduler
/// ([`crate::scheduler`]) free of any Koan type: it decides *when*, this decides *how*. Errors
/// carry no value and finalize bare through [`finalize_error`] instead.
///
/// Peer of [`copy_carried`](super::lift::copy_carried): both are Done-boundary workload hooks.
pub(in crate::machine::execute) trait NodeFinalize {
    /// A satisfying non-union object re-stamps to the declared type **in place, in the producer's
    /// own region** ([`Delivered::restamp_in_place`](crate::witnessed::Delivered::restamp_in_place)):
    /// no bytes move and residence is unchanged, because the delivery walk that consumes the
    /// envelope is what moves the value. A union return and a type value pass through un-restamped;
    /// a mismatch raises.
    ///
    /// `home` is the owner of the region the value lives in — the slot's anchor owner, the same one
    /// the envelope was sealed under. It is passed in rather than read off the envelope: the
    /// envelope's members are one flat antichain in which home is an ordinary member, and the
    /// re-stamp's destination must be exactly the region the value already resides in.
    ///
    /// Nothing here splits the carrier from its coverage — the terminal leaves as a whole envelope,
    /// so no call site can pair one with a coverage that is not its own.
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
        // envelope on verbatim and restamp re-stamps in place in the producer's own region, so the
        // reach the scheduler's delivery walk adopts against is always the reach of the value it
        // stores.
        let Some(obligation) = contract else {
            return Ok(envelope);
        };
        let Some((declared, per_call)) = obligation.declared() else {
            return Ok(envelope);
        };
        let registries = self.ambient.registries();
        let types = &registries.types;
        // Classifying under the envelope's own host pin keeps this a pure read: no relocation,
        // nothing allocated until the disposition is known.
        let disposition = envelope.open(|carried| match carried {
            Carried::Object(object) => {
                if !declared.matches_value(object, registries) {
                    Disposition::Mismatch(object.ktype().name(registries))
                } else if object.embeds_substrate()
                    && !matches!(types.node(declared), TypeNode::Union { .. })
                {
                    // Only a substrate carrier carries a re-stampable type tag, and a declared
                    // union return keeps its runtime type for union-elimination dispatch.
                    Disposition::Restamp
                } else {
                    Disposition::PassThrough
                }
            }
            Carried::Type(t) => {
                if declared.matches_type(t, types) {
                    Disposition::PassThrough
                } else {
                    Disposition::Mismatch(t.name(registries))
                }
            }
            Carried::UnresolvedType(ti) => {
                Disposition::Mismatch(crate::machine::model::render_label(ti.symbol(), registries))
            }
        });
        match disposition {
            Disposition::Mismatch(got) => Err(return_type_mismatch(
                declared,
                per_call,
                obligation.label(),
                got,
                registries,
            )),
            Disposition::PassThrough => Ok(envelope),
            // `home` is the region the value already resides in, so the re-mint leaves residence
            // unchanged and the product envelope covers exactly what the input did.
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

/// Label a `Done`-step **error** with its return contract's trace frame. An error carries no value,
/// so it needs no witness and no declared-return check — it never reaches
/// [`finalize_terminal`](NodeFinalize::finalize_terminal).
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

/// Discharge a tail-spliced slot's residual declared-return obligation against the spliced
/// producer's delivered value, **without** re-stamping: the value stays the producer's. Nothing is
/// adopted here, so the value can be taken live at the caller's read brand rather than as a carrier
/// of its own.
pub(in crate::machine::execute) fn check_spliced_return(
    obligation: &ReturnObligation,
    carried: Carried<'_>,
    registries: &RunRegistries,
) -> Result<(), KError> {
    let types = &registries.types;
    let Some((declared, per_call)) = obligation.declared() else {
        return Ok(());
    };
    let label = obligation.label();
    let matched = match carried {
        Carried::Object(object) => declared.matches_value(object, registries),
        Carried::Type(t) => declared.matches_type(t, types),
        // Every delivered result is resolved; an unlowered name satisfies no contract.
        Carried::UnresolvedType(_) => false,
    };
    if matched {
        return Ok(());
    }
    let got = match carried {
        Carried::Object(object) => object.ktype().name(registries),
        Carried::Type(t) => t.name(registries),
        Carried::UnresolvedType(ti) => crate::machine::model::render_label(ti.symbol(), registries),
    };
    Err(return_type_mismatch(
        declared, per_call, label, got, registries,
    ))
}

#[cfg(test)]
mod tests;

/// The labelled `TypeMismatch` a failed declared-return check raises.
fn return_type_mismatch(
    declared: KType,
    per_call: bool,
    label: &str,
    got: String,
    registries: &RunRegistries,
) -> KError {
    let expected = if per_call {
        format!("{} (per-call return type)", declared.name(registries))
    } else {
        declared.name(registries)
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
