use std::rc::Rc;

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::RegionBrand;
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, KType};
use crate::machine::{CallFrame, FrameSet, KError, KErrorKind};
use crate::witnessed::{reattachable, Witnessed};

use super::runtime::KoanRuntime;

/// `Reattachable` carrier family for a declared-return re-stamp's two region-resident operands: the
/// contract's home region the re-tagged value lands in, and the declared `KType` it is stamped with.
/// Both live in the home region (a strict ancestor of the producer frame) the finalized carrier's
/// witness already pins via its `outer` chain, so [`finalize_terminal`](NodeFinalize::finalize_terminal)
/// folds them in with
/// [`merge`](Witnessed::merge) — the re-stamp is born co-located, no asserted bundle. Layout-invariant:
/// a `(RegionBrand<'r>, &'r KType<'r>)` is two thin pointers whose representation never depends on `'r`.
struct ContractHomeFamily;

reattachable!(ContractHomeFamily => (RegionBrand<'r>, &'r KType<'r>));

/// The workload's Done-boundary contract hook: seal a finished node's **value** terminal against its
/// declared return contract, returning the slot's final terminal. The driver opens the slot's
/// contract at the step brand (alongside the continuation, via
/// [`SealedExtern::open`](crate::witnessed::SealedExtern::open)) and hands this hook the live
/// [`ReturnContract`] plus the (optional) per-call frame. The scheduler decides *when* (the Done
/// boundary); this hook owns the `ReturnContract`- / `KType`-naming *how*, so the scheduler core names
/// neither. Errors carry no value and finalize bare through [`finalize_error`], which never reaches
/// this hook.
///
/// Peer of [`relocate_carried`](super::lift::relocate_carried): both are workload hooks the Done
/// boundary calls. The lift relocates a value across a dep edge; finalize seals the terminal against
/// the return contract. They stay separate — the contract layer is never folded into the lift (see
/// [`lift`](super::lift)).
///
/// A Koan-typed workload hook: the generic scheduler ([`crate::scheduler`]) drives the Done
/// boundary through this trait and names no Koan type itself.
///
/// The terminal arrives **already witnessed** — a lifetime-free [`Witnessed`] carrier — so nothing is
/// erased here; the declared-return re-stamp runs at the merge brand, where the carrier value and the
/// contract's home-region declared type meet.
pub(in crate::machine::execute) trait NodeFinalize {
    /// Seal the slot's value terminal: a [`Witnessed`] carrier the producer built inside its witness
    /// closure, naming its *foreign* reach. Fold the producing frame into that foreign-only witness —
    /// the scope-reach seal at close, the pin a value born under the empty set (the brand-confined
    /// alloc surface, or a region-pure [`resident`](Witnessed::resident)) relies on — and then enforce
    /// the declared return: with no declared type the producer-folded carrier passes through; a
    /// declared-return re-stamp re-tags the value into the contract's home region via
    /// [`merge`](Witnessed::merge), re-sealed under that same witness (which pins `home` through its
    /// `outer` chain). The fold is idempotent for a carrier that already names its dest frame, so
    /// existing construction terminals seal unchanged. A `None` frame (a frameless / run producer) has
    /// no frame to fold and seals as-is.
    fn finalize_terminal<'o>(
        &self,
        carrier: Witnessed<CarriedFamily, FrameSet>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError>;
}

impl NodeFinalize for KoanRuntime<'_> {
    fn finalize_terminal<'o>(
        &self,
        carrier: Witnessed<CarriedFamily, FrameSet>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError> {
        // A frameless / run producer carries no per-call return obligation (the run_loop frame-gates
        // the contract to `None` here anyway) and no producer frame to fold: its backing already
        // outlives the carrier, so the foreign-reach-only witness is the exact reach. Seal as-is.
        let Some(producer) = frame else {
            return Ok(carrier);
        };
        // The scope-reach seal at close: fold the producing frame into the carrier's foreign-only
        // witness — the pin that makes a value born under the empty set (the brand-confined alloc
        // surface) storable. Applied before the pass-through / re-stamp split so both carry it: the
        // re-stamp's `home_carrier` inherits the folded witness, keeping the re-homed value pinned.
        // Idempotent for a carrier that already names its dest frame (producer == dest, subsumed), so
        // the existing dest-witnessed construction terminals seal unchanged.
        let carrier = carrier.reseal_under(FrameSet::singleton(producer.storage_rc()));
        // No declared return (or a non-`Resolved` FN-def carrier): pass through — the producer-folded
        // witness is the exact reach, no asserted bundle.
        let Some((declared, label, per_call)) = pull_declared_return(contract) else {
            return Ok(carrier);
        };
        // Check the declared return at the merge brand, where the carrier value and the declared type
        // — folded in from the contract's home region — meet at one `'b`. This is the only place a
        // `&KType<'o>` and the lifetime-free carrier can be compared: `KType` is invariant in its
        // lifetime, so a free-`'o` declared type and a branded carrier value cannot be compared
        // outside a shared brand. The home region is a strict ancestor the carrier's witness already
        // pins via its `outer` chain, so the union re-seals under the carrier's own witness
        // (subsumption drops the home duplicate). The **object** channel coarsens/re-stamps into the
        // home region (e.g. `List<Number>` through `:(LIST OF Any)`); the **type** channel checks but
        // passes the value through unchanged — it never re-tags. A failed check is captured and raised
        // after the fold (the discarded re-home is harmless).
        let home = contract
            .expect("a declared return type implies a contract")
            .home_region();
        let home_carrier = Witnessed::<ContractHomeFamily, FrameSet>::new(
            (home, declared),
            carrier.witness().clone(),
        );
        let mut mismatch: Option<KError> = None;
        let checked = carrier
            .merge::<ContractHomeFamily, CarriedFamily>(
                home_carrier,
                |value, (home_region, declared_type), _brand| match value {
                    Carried::Object(_) => {
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
            )
            .expect("a FrameSet set witness always represents the union");
        match mismatch {
            Some(error) => Err(error),
            None => Ok(checked),
        }
    }
}

/// Label a `Done`-step **error** with its frame-gated return contract's trace frame — the callee's
/// declared-return frame — and return it for a bare finalize. An error carries no value, so it needs
/// no witness and no declared-return re-stamp (the value check / coarsen lives in
/// [`finalize_terminal`](NodeFinalize::finalize_terminal), which errors never reach). A `None` frame
/// (a frameless slot or the non-dying run frame) carries no per-call return obligation, so the error
/// passes through unlabelled. Reads no scope: the contract rides the step open, witnessed by the cart
/// `Rc`.
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
/// non-`Resolved` (a `Deferred` carrier still in its FN-def signature). [`finalize_terminal`] reads it
/// so the value check can run *inside* the re-stamp `merge` (where the carrier value and the declared
/// type meet at one brand).
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
