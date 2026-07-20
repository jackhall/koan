use std::rc::Rc;

use crate::machine::core::ReturnContract;
use crate::machine::core::{FoldingBrand, KoanStorageProfile};
use crate::machine::model::CarriedFamily;
use crate::machine::model::{Carried, KType, TypeRegistry};
use crate::machine::{CarrierWitness, DeliveredCarried, FrameSet, KError, KErrorKind};
use crate::witnessed::{reattachable, RegionHandle, Residence, Sealed, SealedExtern, Witnessed};

use super::obligation::ReturnObligation;
use super::runtime::KoanRuntime;

/// `Reattachable` carrier family for a declared-return re-stamp's two home-region operands: the
/// contract's home region and the declared `KType`. Both live in the home region, into which
/// [`finalize_terminal`](NodeFinalize::finalize_terminal) re-homes the checked value through the
/// delivery envelope's [`transfer_into`](crate::witnessed::Delivered::transfer_into) — minting the value's reach (and,
/// for a home-borrowing value, its producer frame) into that region's arena so the re-homed value's
/// carrier names everything it reaches. Layout-invariant: `(RegionHandle<'r, _>, &'r KType)` is
/// two thin pointers whose representation never depends on `'r`.
struct ContractHomeFamily;

reattachable!(ContractHomeFamily => (RegionHandle<'r, KoanStorageProfile>, &'r KType));

/// Seal a finished node's **value** terminal against its declared return contract, returning the
/// slot's final terminal. This hook receives the value already sealed into a delivery envelope
/// (pinned by the slot's anchor region owner) plus the step's [`ReturnObligation`], and opens the
/// obligation's self-pinning cell to recover the live [`ReturnContract`]. The scheduler decides
/// *when* (the Done boundary) and hands over the sealed envelope; this hook owns the
/// `ReturnContract`/`KType` *how*, so the generic scheduler ([`crate::scheduler`]) names no Koan
/// type. Errors carry no value and finalize bare through [`finalize_error`], which never reaches
/// this hook.
///
/// Peer of [`copy_carried`](super::lift::copy_carried): both are Done-boundary workload hooks.
///
/// The envelope is sealed by the caller, so this hook calls neither
/// [`storage_rc`](crate::machine::CallFrame) nor [`Delivered::seal`](crate::witnessed::Delivered::seal): it supplies only the
/// declared-return re-stamp fold, run at a shared brand where the carrier value and the contract's
/// home-region declared type meet.
pub(in crate::machine::execute) trait NodeFinalize {
    /// Seal the slot's value terminal against its declared return. With no declared return (or a
    /// framed producer with no obligation) the envelope's carrier recovers **as-is** through the
    /// seal→unseal round-trip — the scheduler's retention hold keeps the producer frame alive until
    /// every destination pulls, so the value keeps residing where it was born. A declared-return
    /// re-stamp re-tags an **object** value into the contract's home region through the envelope
    /// transfer (the value comes to reside there; its reach — and, when it borrows into the dying
    /// producer, that producer — is minted into the home arena); a **type** value is checked at a
    /// shared brand and passes through un-relocated.
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
        // obligation): recover the sealed carrier as-is via the seal→unseal round-trip — retention
        // owns the frame's lifetime, so the Done boundary makes no memory decision.
        let Some(obligation) = contract else {
            return Ok(envelope.into_cell().unseal());
        };
        // Open the obligation's self-pinning cell to recover the live contract: its own `FrameSet`
        // witness pins the home-region owner, so no external pin is needed. The whole declared-return
        // check runs inside this brand; nothing branded by it escapes into the returned terminal. The
        // envelope is captured by value — each path through the closure consumes or recovers it
        // exactly once (the miss unseals it, the object channel transfers it, the type channel unseals
        // its cell), and the paths are mutually exclusive returns.
        obligation.open_cell(move |live| {
            let Some((declared, per_call)) = pull_declared_return(live) else {
                return Ok(envelope.into_cell().unseal());
            };
            let label = obligation.label();
            // The envelope's retained host is the anchor's own region owner — the pin the value is
            // read (and relocated) under, and the same owner the run loop seeds as the slot's
            // retention host.
            let producer_pin = Rc::clone(envelope.host());
            let home = live.home_region();
            let types = self.ambient.type_registry();
            let is_object = envelope.open(|carried| matches!(carried, Carried::Object(_)));
            if is_object {
                let mut mismatch: Option<KError> = None;
                // The **object** channel coarsens/re-stamps into the home region: a genuine
                // relocation, run through the envelope transfer at `Residence::Copied` — the value's
                // reach is minted into the home arena, and the dying producer materializes as a
                // member only when the value's borrows genuinely reach it (`borrows_host`); a
                // region-pure result leaves the producer to retention alone, releasing it at
                // pull-count zero. The home-region operand rides `resident` (the empty carrier): its
                // backing — the home region and the declared type in it — stays live across the call
                // via the obligation's cell pin. Accepted residual: a failed type check still leaves
                // its minted set in the home arena — the value was genuinely relocated before the
                // check failed, and the path returns the `Err` terminal.
                let home_operand: Witnessed<ContractHomeFamily, CarrierWitness> =
                    Witnessed::resident((home.handle(), declared));
                let checked = envelope
                    .transfer_into_placing::<ContractHomeFamily, CarriedFamily, _>(
                        home_operand,
                        Residence::Copied,
                        |value, (_home_region, declared_type), placement| {
                            let home_region = FoldingBrand::in_fold_closure(placement);
                            let object = value.object();
                            if !declared_type.matches_value(object, types) {
                                mismatch = Some(return_type_mismatch(
                                    declared_type,
                                    per_call,
                                    label,
                                    object.ktype().name(),
                                ));
                                return Carried::Object(
                                    home_region.alloc_object_folded(object.deep_clone()),
                                );
                            }
                            // A declared union return checks (above) but never re-tags: the value keeps
                            // its own runtime type, which is what union elimination dispatches on. Every
                            // other declared return re-stamps the value into the declared type.
                            if matches!(declared_type, KType::Union { .. }) {
                                return Carried::Object(
                                    home_region.alloc_object_folded(object.deep_clone()),
                                );
                            }
                            Carried::Object(
                                home_region.alloc_object_folded(
                                    object.deep_clone().stamp_type(declared_type),
                                ),
                            )
                        },
                    );
                return match mismatch {
                    Some(error) => Err(error),
                    None => Ok(checked),
                };
            }
            // The **type** channel checks but never relocates: the value keeps its residence and its
            // carrier verbatim. The value passes through on a match; a mismatch raises. The
            // brand-unifying comparison lives in the shared [`match_declared_return`].
            let sealed = Sealed::seal(envelope.into_cell().unseal());
            let value_cell: SealedExtern<CarriedFamily> = SealedExtern::seal(*sealed.erased());
            let pin = match live.home_owner() {
                Some(owner) => FrameSet::union(
                    &FrameSet::singleton(producer_pin),
                    &FrameSet::singleton(owner),
                ),
                // A released home owner (the capture's `Weak` already dropped): the home region is
                // still live for this synchronous check — the contract's own cell pins it across
                // this open — so the producer pin alone rides the zip open.
                None => FrameSet::singleton(producer_pin),
            };
            match match_declared_return(
                value_cell,
                home.handle(),
                declared,
                &pin,
                per_call,
                label,
                types,
            ) {
                Some(error) => Err(error),
                None => Ok(sealed.unseal()),
            }
        })
    }
}

/// Label a `Done`-step **error** with its return contract's trace frame and return it for a bare
/// finalize. An error carries no value, so it needs no witness and no declared-return re-stamp (that
/// check lives in [`finalize_terminal`](NodeFinalize::finalize_terminal), which errors never reach). A
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

/// Pull the declared return type off the live `contract` plus its `per_call` flag, or `None` when
/// nothing is declared — a `Function` whose signature return is non-`Resolved` (a `Deferred` carrier
/// still in its FN-def signature). The diagnostic label rides the [`ReturnObligation`] instead, read
/// via [`ReturnObligation::label`].
fn pull_declared_return<'o>(contract: ReturnContract<'o>) -> Option<(&'o KType, bool)> {
    match contract {
        ReturnContract::Function(f) => match &f.signature.return_type {
            crate::machine::model::ReturnType::Resolved(d) => Some((d, false)),
            _ => None,
        },
        ReturnContract::Arm { ret, .. } => Some((ret, false)),
        ReturnContract::PerCall { ret, .. } => Some((ret, true)),
    }
}

/// Zip-open a value carrier against the declared return type under `pin` — the shared brand where
/// the invariant `KType` and the lifetime-free value meet — and match by channel: an object against
/// [`matches_value`](KType::matches_value), a type against [`matches_type`](KType::matches_type). No
/// relocation; the value keeps its residence. Returns the labelled mismatch, or `None` on a pass.
/// `KType` is invariant in its lifetime, so the branded declared type and the erased carrier can
/// only be compared under one shared brand, pinned by `pin` (the value's backing unioned with the
/// contract's home owner). Shared by [`finalize_terminal`](NodeFinalize::finalize_terminal)'s type
/// channel and the tail-splice checker micro-step ([`check_spliced_return`]).
fn match_declared_return<'c>(
    value_cell: SealedExtern<CarriedFamily>,
    home_handle: RegionHandle<'c, KoanStorageProfile>,
    declared: &'c KType,
    pin: &FrameSet,
    per_call: bool,
    label: &str,
    types: &TypeRegistry,
) -> Option<KError> {
    let contract_operand = SealedExtern::<ContractHomeFamily>::erase((home_handle, declared));
    let mut mismatch: Option<KError> = None;
    value_cell
        .zip(contract_operand)
        .open(pin, |(value, (_home_region, declared_type))| {
            let matched = match value {
                Carried::Object(object) => declared_type.matches_value(object, types),
                Carried::Type(t) => declared_type.matches_type(t),
                // Every delivered result is resolved; an unlowered name satisfies no contract.
                Carried::UnresolvedType(_) => false,
            };
            if !matched {
                let got = match value {
                    Carried::Object(object) => object.ktype().name(),
                    Carried::Type(t) => t.name(),
                    Carried::UnresolvedType(ti) => ti.render(),
                };
                mismatch = Some(return_type_mismatch(declared_type, per_call, label, got));
            }
        });
    mismatch
}

/// Discharge a tail-spliced slot's residual declared-return obligation against the spliced producer's
/// terminal, WITHOUT relocating it — the checker micro-step's check. Opens the obligation's
/// self-pinning cell, pulls the declared return, and runs the shared [`match_declared_return`] against
/// the producer's delivered value; the value stays the producer's, relocated only later when the
/// re-emitted `Forward` finalizes through [`NodeStep::ForwardReady`](super::nodes::NodeStep). Returns
/// the labelled mismatch or `Ok(())`.
pub(in crate::machine::execute) fn check_spliced_return(
    obligation: &ReturnObligation,
    delivered: &DeliveredCarried,
    types: &TypeRegistry,
) -> Result<(), KError> {
    obligation.open_cell(|live| {
        let Some((declared, per_call)) = pull_declared_return(live) else {
            return Ok(());
        };
        let label = obligation.label();
        let home = live.home_region();
        // The value carrier and its liveness pin come from the producer's delivery envelope
        // (duplicated — the producer keeps its terminal for the re-emitted `Forward`): the pin is the
        // retained producer host, unioned with the contract's home owner (a released home owner
        // stays live for this synchronous check via the obligation's own cell pin).
        let sealed = Sealed::seal(delivered.duplicate().into_cell().unseal());
        let value_cell: SealedExtern<CarriedFamily> = SealedExtern::seal(*sealed.erased());
        let pin = match live.home_owner() {
            Some(owner) => FrameSet::union(
                &FrameSet::singleton(Rc::clone(delivered.host())),
                &FrameSet::singleton(owner),
            ),
            None => FrameSet::singleton(Rc::clone(delivered.host())),
        };
        match match_declared_return(
            value_cell,
            home.handle(),
            declared,
            &pin,
            per_call,
            label,
            types,
        ) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
}

#[cfg(test)]
mod tests;

/// The labelled `TypeMismatch` a failed declared-return check raises. `expected` names the declared
/// type (tagged "per-call return type" for a `PerCall`); `got` names the produced carrier.
fn return_type_mismatch(declared: &KType, per_call: bool, label: &str, got: String) -> KError {
    let expected = if per_call {
        format!("{} (per-call return type)", declared.name())
    } else {
        declared.name()
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
