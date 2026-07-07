use std::rc::Rc;

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::RegionBrand;
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, KType};
use crate::machine::{CallFrame, CarrierWitness, FrameStorage, KError, KErrorKind};
use crate::witnessed::{reattachable, Erased, SetWitness, Witnessed};

use super::lift::sever_residence;
use super::runtime::KoanRuntime;

/// `Reattachable` carrier family for a declared-return re-stamp's two home-region operands: the
/// contract's home region and the declared `KType`. Both live in the home region, into which
/// [`finalize_terminal`](NodeFinalize::finalize_terminal) re-homes the checked value with
/// [`merge`](Witnessed::merge) — pinning that region's owner on the result so the re-homed value
/// outlives the released producer frame. Layout-invariant: `(RegionBrand<'r>, &'r KType<'r>)` is two
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
/// Peer of [`copy_carried`](super::lift::copy_carried): both are Done-boundary workload hooks, but
/// the contract layer is never folded into the lift (see [`lift`](super::lift)).
///
/// The terminal arrives **already witnessed** (a lifetime-free [`Witnessed`] carrier), so nothing is
/// erased here; the declared-return re-stamp runs at the merge brand, where the carrier value and the
/// contract's home-region declared type meet.
pub(in crate::machine::execute) trait NodeFinalize {
    /// Seal the slot's value terminal against its dying producer frame. With no declared return, the
    /// Done-boundary gate severs a region-pure value's residence (releasing the frame) and passes a
    /// frame-borrowing value through unchanged. A declared-return re-stamp re-tags the value into the
    /// contract's home region via [`merge`](Witnessed::merge) and pins that home region's owner, so a
    /// region-pure result is severed off the producer frame too and only a frame-borrowing value keeps
    /// it. A `None` frame (frameless / run producer) seals as-is.
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
        // `None`) and no producer frame to fold: its backing already outlives the carrier. Seal as-is.
        let Some(producer) = frame else {
            return Ok(carrier);
        };
        // No declared return (or a non-`Resolved` FN-def carrier): the Done-boundary gate. If the
        // carrier's **reach** already covers the producer, the value genuinely borrows into the dying
        // frame — the reach names it, so seal as-is (residence pinned via reach, no fold needed).
        // Otherwise the value borrows nothing into the frame, so sever its residence: copy the top node
        // into an owned backing and release the frame. A fully-owned scalar thus seals with empty reach.
        let Some((declared, label, per_call)) = pull_declared_return(contract) else {
            let carrier = if carrier.witness().reach_covers(producer.region()) {
                carrier
            } else {
                sever_residence(carrier, producer)
            };
            return Ok(carrier);
        };
        // Declared-return re-stamp path: the re-stamp deep-clones the value into the contract's home
        // region below, so the checked value comes to reside there — pinned by the home region's owner,
        // which the `home_carrier` operand below folds in via the merge composition. The producer frame
        // is then released for a region-pure result: sever its residence here (copy the top node into an
        // owned backing), so the merge below re-homes from that backing with the producer free. A value
        // that genuinely borrows into the producer (its reach names it) keeps it — its interior borrows
        // survive the re-stamp verbatim (folded as reach onto the merged result). When the home owner
        // can't be resolved (a MATCH/TRY arm, or a released capture) the producer stays pinned (sound
        // over-retention) instead — a region-pure carrier is rehosted onto it directly (nothing else
        // proves the producer's region alive otherwise); an already-hosted carrier needs no change,
        // since a downstream fold materializes its host as reach the same way regardless.
        let home_owner = declared_return_home_owner(contract);
        let carrier = if home_owner.is_some() && !carrier.witness().reach_covers(producer.region())
        {
            sever_residence(carrier, producer)
        } else if matches!(carrier.witness(), CarrierWitness::Empty) {
            carrier.rehost(CarrierWitness::singleton(producer.storage_rc()))
        } else {
            carrier
        };
        // Check the declared return at the merge brand, where the carrier value and the home-region
        // declared type meet at one `'b`. `KType` is invariant in its lifetime, so a free-`'o` declared
        // type and the lifetime-free carrier can only be compared under a shared brand. The **object**
        // channel coarsens/re-stamps into the home region; the **type** channel checks but passes the
        // value through unchanged. A failed check is captured and raised after the fold.
        let home = contract
            .expect("a declared return type implies a contract")
            .home_region();
        // Bundle `home` / `declared` witnessed by the home owner when resolvable — so the merge below
        // mints `carrier`'s existing reach (and its own host, if foreign) into the home region's arena,
        // re-homing the composed result onto the home owner in one step (the sound-over-retention comment
        // above: a `Carried::Type` passthrough that never actually moves there keeps its true residence
        // alive as a folded-in reach member instead). `resident` (unwitnessed) when no owner is
        // resolvable — the merge then degrades to `merge(w, ∅) == w`, leaving `carrier`'s own witness
        // (already pinned to the producer above) untouched.
        let home_carrier: Witnessed<ContractHomeFamily, CarrierWitness> = match &home_owner {
            Some(owner) => Witnessed::from_erased(
                Erased::erase((home, declared)),
                CarrierWitness::singleton(Rc::clone(owner)),
            ),
            None => Witnessed::resident((home, declared)),
        };
        // Cloned *before* the merge so it survives the merge's unconditional drop of `carrier`'s own
        // witness regardless of which branch below runs — a cheap `Rc` bump (`Hosted.host`) or an
        // owned-reach clone (`Severed`). The **type** branch needs it: `value` passes through
        // unrelocated, still pointing at whatever `carrier` witnessed before the merge (a `Severed`
        // backing, e.g.) — the merge's own composed witness only accounts for a *relocated* value
        // (it mints reach forward but drops a `Severed.node`, since every other call site's
        // projection copies), so trusting it here for a passthrough would free the very backing
        // `value` still reads through. `moved` picks between the two after the fact.
        let original_witness = carrier.witness().clone();
        let mut moved = false;
        let mut mismatch: Option<KError> = None;
        let checked = carrier.merge::<ContractHomeFamily, CarriedFamily>(
            home_carrier,
            |value, (home_region, declared_type), _brand| match value {
                Carried::Object(_) => {
                    moved = true;
                    let object = value.object();
                    if !declared_type.matches_value(object) {
                        mismatch = Some(return_type_mismatch(
                            declared_type,
                            per_call,
                            &label,
                            object.ktype().name(),
                        ));
                        return Carried::Object(home_region.alloc_object(object.deep_clone()));
                    }
                    Carried::Object(
                        home_region.alloc_object(object.deep_clone().stamp_type(declared_type)),
                    )
                }
                Carried::Type(t) => {
                    if !declared_type.matches_type(t) {
                        mismatch = Some(return_type_mismatch(
                            declared_type,
                            per_call,
                            &label,
                            t.name(),
                        ));
                    }
                    value
                }
            },
        );
        // The **object** branch relocated: the merge-composed witness (home owner ∪ `carrier`'s old
        // reach) is correct and is what `checked` already carries. The **type** branch didn't: restore
        // the pre-merge witness verbatim, since `value` still reads through whatever it witnessed.
        let checked = if moved {
            checked
        } else {
            checked.rehost(original_witness)
        };
        match mismatch {
            Some(error) => Err(error),
            None => Ok(checked),
        }
    }
}

/// The `Rc<FrameStorage>` owning a declared-return contract's home region — the region a re-stamp
/// re-homes its checked value into. Resolvable for a `Function` / `PerCall` (the callee's captured-scope
/// region owner, live under the open's witness for the whole call); a MATCH / TRY `Arm` carries only a
/// [`RegionBrand`] with no owner handle, so it returns `None` and the caller keeps the producer frame
/// pinned (sound over-retention).
fn declared_return_home_owner(contract: Option<ReturnContract<'_>>) -> Option<Rc<FrameStorage>> {
    match contract {
        Some(ReturnContract::Function(f)) | Some(ReturnContract::PerCall { func: f, .. }) => {
            f.captured_scope().region_owner().upgrade()
        }
        _ => None,
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
