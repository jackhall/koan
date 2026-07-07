use std::rc::Rc;

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::RegionBrand;
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, KType};
use crate::machine::{CallFrame, CarrierWitness, FrameSet, KError, KErrorKind};
use crate::witnessed::{reattachable, Delivered, Residence, Sealed, SealedExtern, Witnessed};

use super::runtime::KoanRuntime;

/// `Reattachable` carrier family for a declared-return re-stamp's two home-region operands: the
/// contract's home region and the declared `KType`. Both live in the home region, into which
/// [`finalize_terminal`](NodeFinalize::finalize_terminal) re-homes the checked value through the
/// delivery envelope's [`transfer_into`](Delivered::transfer_into) — minting the value's reach (and,
/// for a home-borrowing value, its producer frame) into that region's arena so the re-homed value's
/// carrier names everything it reaches. Layout-invariant: `(RegionBrand<'r>, &'r KType<'r>)` is two
/// thin pointers whose representation never depends on `'r`.
struct ContractHomeFamily;

reattachable!(ContractHomeFamily => (RegionBrand<'r>, &'r KType<'r>));

/// Seal a finished node's **value** terminal against its declared return contract, returning the
/// slot's final terminal. The driver opens the slot's contract at the step brand (via
/// [`SealedExtern::open`](crate::witnessed::SealedExtern::open)) and hands this hook the live
/// [`ReturnContract`] plus the optional per-call frame. The scheduler decides *when* (the Done
/// boundary); this hook owns the `ReturnContract`/`KType` *how*, so the generic scheduler
/// ([`crate::scheduler`]) names no Koan type. Errors carry no value and finalize bare through
/// [`finalize_error`], which never reaches this hook.
///
/// Peer of [`copy_carried`](super::lift::copy_carried): both are Done-boundary workload hooks.
///
/// The terminal arrives **already witnessed** (a lifetime-free [`Witnessed`] carrier), so nothing is
/// erased here; the declared-return re-stamp runs at a shared brand, where the carrier value and the
/// contract's home-region declared type meet.
pub(in crate::machine::execute) trait NodeFinalize {
    /// Seal the slot's value terminal against its declared return. With no declared return the
    /// carrier seals **as-is** — the scheduler's retention hold keeps the producer frame alive until
    /// every destination pulls, so nothing severs and no residence moves. A declared-return
    /// re-stamp re-tags an **object** value into the contract's home region through the envelope
    /// transfer (the value comes to reside there; its reach — and, when it borrows into the dying
    /// producer, that producer — is minted into the home arena); a **type** value is checked at a
    /// shared brand and passes through un-relocated. A `None` frame (frameless / run producer)
    /// seals as-is.
    fn finalize_terminal<'o>(
        &self,
        carrier: Witnessed<CarriedFamily, CarrierWitness>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError>;
}

impl NodeFinalize for KoanRuntime<'_> {
    fn finalize_terminal<'o>(
        &self,
        carrier: Witnessed<CarriedFamily, CarrierWitness>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        // A frameless / run producer has no per-call return obligation (the contract is gated to
        // `None`); a framed producer with no declared return seals as-is too — retention owns the
        // frame's lifetime, so the Done boundary makes no memory decision.
        let Some(producer) = frame else {
            return Ok(carrier);
        };
        let Some((declared, label, per_call)) = pull_declared_return(contract) else {
            return Ok(carrier);
        };
        // Declared-return path. The producer frame's storage is the pin the value is read (and
        // relocated) under — the same owner the run loop seeds as the slot's retention host.
        let producer_pin = producer.storage_rc();
        let home = contract
            .expect("a declared return type implies a contract")
            .home_region();
        let envelope: Delivered<CarriedFamily, CarrierWitness, _> =
            Delivered::seal(carrier, Rc::clone(&producer_pin));
        let is_object = envelope.open(|carried| matches!(carried, Carried::Object(_)));
        let mut mismatch: Option<KError> = None;
        if is_object {
            // The **object** channel coarsens/re-stamps into the home region: a genuine relocation,
            // run through the envelope transfer at `Residence::Copied` — the value's reach is minted
            // into the home arena, and the dying producer materializes as a member only when the
            // value's borrows genuinely reach it (`borrows_host`); a region-pure result leaves the
            // producer to retention alone, releasing it at pull-count zero. The home-region operand
            // rides `resident` (the empty carrier): its backing — the home region and the declared
            // type in it — stays live across the call via the step's contract pin. Accepted
            // residual: a failed type check still leaves its minted set in the home arena — the
            // value was genuinely relocated before the check failed, and the path returns the `Err`
            // terminal.
            let home_operand: Witnessed<ContractHomeFamily, CarrierWitness> =
                Witnessed::resident((home, declared));
            let checked = envelope.transfer_into::<ContractHomeFamily, CarriedFamily, _>(
                home_operand,
                Residence::Copied,
                |value, (home_region, declared_type), _brand| {
                    let object = value.object();
                    if !declared_type.matches_value(object) {
                        mismatch = Some(return_type_mismatch(
                            declared_type,
                            per_call,
                            &label,
                            object.ktype().name(),
                        ));
                        return Carried::Object(
                            home_region.alloc_object(object.deep_clone()),
                        );
                    }
                    Carried::Object(
                        home_region.alloc_object(object.deep_clone().stamp_type(declared_type)),
                    )
                },
            );
            return match mismatch {
                Some(error) => Err(error),
                None => Ok(checked),
            };
        }
        // The **type** channel checks but never relocates: the value keeps its residence and its
        // carrier verbatim. `KType` is invariant in its lifetime, so the free-`'o` declared type and
        // the lifetime-free carrier can only be compared under one shared brand: both are erased and
        // zip-opened together, pinned by the producer (the value's backing) unioned with the
        // contract's home owner (the declared type's backing). A failed check is captured and raised
        // after the open.
        let sealed = Sealed::seal(envelope.into_cell().unseal());
        let value_cell: SealedExtern<CarriedFamily> = SealedExtern::seal(*sealed.erased());
        let contract_operand = SealedExtern::<ContractHomeFamily>::erase((home, declared));
        let pin = match contract.and_then(ReturnContract::home_owner) {
            Some(owner) => FrameSet::union(
                &FrameSet::singleton(producer_pin),
                &FrameSet::singleton(owner),
            ),
            // A released home owner (the capture's `Weak` already dropped): the home region is still
            // live for this synchronous check — the contract itself was opened at the step brand
            // under the step's own pins — so the producer pin alone rides the open.
            None => FrameSet::singleton(producer_pin),
        };
        value_cell
            .zip(contract_operand)
            .open(&pin, |(value, (_home_region, declared_type))| {
                if let Carried::Type(t) = value {
                    if !declared_type.matches_type(t) {
                        mismatch = Some(return_type_mismatch(
                            declared_type,
                            per_call,
                            &label,
                            t.name(),
                        ));
                    }
                }
            });
        match mismatch {
            Some(error) => Err(error),
            None => Ok(sealed.unseal()),
        }
    }
}

/// Label a `Done`-step **error** with its return contract's trace frame and return it for a bare
/// finalize. An error carries no value, so it needs no witness and no declared-return re-stamp (that
/// check lives in [`finalize_terminal`](NodeFinalize::finalize_terminal), which errors never reach). A
/// `None` frame carries no per-call return obligation, so the error passes through unlabelled.
pub(in crate::machine::execute) fn finalize_error(
    error: KError,
    frame: Option<&Rc<CallFrame>>,
    contract: Option<ReturnContract<'_>>,
) -> KError {
    match (frame, contract) {
        (Some(_), Some(contract)) => {
            let label = match contract {
                ReturnContract::Function(f) => f.summarize(),
                ReturnContract::Arm { kind, .. } => kind.to_string(),
                ReturnContract::PerCall { func, .. } => func.summarize(),
            };
            error.with_frame(crate::machine::TraceFrame::bare(label.clone(), label))
        }
        _ => error,
    }
}

/// Pull the declared return type off `contract` plus its diagnostic label and the `per_call` flag, or
/// `None` when nothing is declared — no contract, or a `Function` whose signature return is
/// non-`Resolved` (a `Deferred` carrier still in its FN-def signature).
fn pull_declared_return<'o>(
    contract: Option<ReturnContract<'o>>,
) -> Option<(&'o KType<'o>, String, bool)> {
    match contract {
        Some(ReturnContract::Function(f)) => match &f.signature.return_type {
            crate::machine::model::types::ReturnType::Resolved(d) => {
                Some((d, f.summarize(), false))
            }
            _ => None,
        },
        Some(ReturnContract::Arm { ret, kind, .. }) => Some((ret, kind.to_string(), false)),
        Some(ReturnContract::PerCall { func, ret }) => Some((ret, func.summarize(), true)),
        None => None,
    }
}

#[cfg(test)]
mod tests;

/// The labelled `TypeMismatch` a failed declared-return check raises. `expected` names the declared
/// type (tagged "per-call return type" for a `PerCall`); `got` names the produced carrier.
fn return_type_mismatch(declared: &KType<'_>, per_call: bool, label: &str, got: String) -> KError {
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
