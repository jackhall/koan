use std::rc::Rc;

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::KoanRegion;
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, KType};
use crate::machine::{CallFrame, FrameSet, KError, KErrorKind};
use crate::witnessed::{reattachable, MergeWitness, Witnessed};

use super::runtime::KoanRuntime;

/// `Reattachable` carrier family for a declared-return re-stamp's two region-resident operands: the
/// contract's home region the re-tagged value lands in, and the declared `KType` it is stamped with.
/// Both live in the home region (a strict ancestor of the producer frame) the finalized carrier's
/// witness already pins via its `outer` chain, so [`finalize_terminal_witnessed`] folds them in with
/// [`merge`](Witnessed::merge) — the re-stamp is born co-located, no asserted bundle. Layout-invariant:
/// a `(&'r KoanRegion, &'r KType<'r>)` is two thin pointers whose representation never depends on `'r`.
struct ContractHomeFamily;

reattachable!(ContractHomeFamily => (&'r KoanRegion, &'r KType<'r>));

/// The workload's Done-boundary contract hook: enforce a finished node's declared return contract,
/// returning the slot's final terminal. The driver opens the slot's contract at the step brand
/// (alongside the continuation, via [`SealedExtern::open`](crate::witnessed::SealedExtern::open)) and
/// hands this hook the live [`ReturnContract`] plus the (optional) per-call frame; the hook runs the
/// declared-return check.
/// The scheduler decides *when* (the Done boundary); this hook owns the `ReturnContract`-/
/// `KType`-naming *how*, so the scheduler core names neither.
///
/// Peer of [`relocate_carried`](super::lift::relocate_carried): both are workload hooks the Done
/// boundary calls. The lift relocates a value across a dep edge; finalize enforces the return
/// contract and bundles the terminal with its witness set. They stay separate — the contract layer
/// is never folded into the lift (see [`lift`](super::lift)).
///
/// A Koan-typed workload hook: the generic scheduler ([`crate::scheduler`]) drives the Done
/// boundary through this trait and names no Koan type itself.
///
/// The value arrives live at the step lifetime `'o` (the same lifetime the contract is opened at), so
/// the declared-return check and coarsening re-tag run while value and contract share `'o`; the
/// checked terminal is then erased into a [`Witnessed`] under its witness set, severing `'o`.
pub(in crate::machine::execute) trait NodeFinalize {
    /// Enforce the declared return on `output` against the already-vended live `contract`, then bundle
    /// the checked terminal with the witness set of every region it reaches: `dep_reached` (the
    /// step's accumulated dep sources) ∪ the producer `frame`. A `None` frame (a frameless slot or the
    /// non-dying run frame) passes the value through untouched, vends no contract, and folds in no
    /// frame — the terminal's backing already outlives the carrier (the dep sources alone pin it).
    fn finalize_terminal<'o>(
        &self,
        output: Result<Carried<'o>, KError>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
        dep_reached: FrameSet,
    ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError>;

    /// The object-family Done-boundary hook: seal a terminal that arrives **already witnessed** — a
    /// [`Witnessed`] carrier the construction inversion built inside its witness closure, naming
    /// every region it reaches. Where [`finalize_terminal`](Self::finalize_terminal) computes a
    /// witness set and bundles a bare value (the transitional type/error path), this hook trusts the
    /// carrier's own witness and only enforces the declared return: with no declared type the carrier
    /// passes through untouched (no `Witnessed::new`); a declared-return re-stamp re-tags the value
    /// into the contract's home region via [`merge`](Witnessed::merge), re-sealed under the carrier's
    /// own witness (which pins `home` through its `outer` chain). A `None` frame (a frameless / run
    /// producer) carries no per-call return obligation and seals as-is.
    fn finalize_terminal_witnessed<'o>(
        &self,
        carrier: Witnessed<CarriedFamily, FrameSet>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError>;
}

impl NodeFinalize for KoanRuntime<'_> {
    fn finalize_terminal<'o>(
        &self,
        output: Result<Carried<'o>, KError>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
        dep_reached: FrameSet,
    ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError> {
        // Check / coarsen while the value is still live at `'o`, then erase it into a `Witnessed`
        // under its witness set. The producer's own `FrameStorage` joins the dep sources — and, by its
        // `outer` chain, pins the coarsening home region (a strict ancestor) too; a frameless / run
        // producer folds in nothing, leaving the surviving dep sources (or the empty set) as the pin.
        let checked = enforce_return_contract(output, frame, contract)?;
        let witness = match frame {
            Some(producer) => {
                FrameSet::merge(&dep_reached, &FrameSet::singleton(producer.storage_rc()))
                    .expect("a set witness always represents the union")
            }
            None => dep_reached,
        };
        Ok(Witnessed::new(checked, witness))
    }

    fn finalize_terminal_witnessed<'o>(
        &self,
        carrier: Witnessed<CarriedFamily, FrameSet>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError> {
        // A frameless / run producer carries no per-call return obligation (the run_loop frame-gates
        // the contract to `None` here anyway): the carrier already names its exact reach, seal as-is.
        if frame.is_none() {
            return Ok(carrier);
        }
        // No declared return (or a non-`Resolved` FN-def carrier): pass through — the carrier's own
        // witness is the exact reach, no asserted bundle.
        let Some((declared, label, per_call)) = pull_declared_return(contract) else {
            return Ok(carrier);
        };
        // Re-tag to the declared return type (may coarsen, e.g. `List<Number>` through
        // `:(LIST OF Any)`). The check and re-stamp run **inside** the `merge`, where the carrier
        // value and the declared type — folded in from the contract's home region — meet at one
        // brand (the only place a `&KType<'o>` and the lifetime-free carrier can be compared). The
        // re-tag homes in the home region, a strict ancestor the carrier's witness already pins via
        // its `outer` chain, so the union re-seals under the carrier's own witness (subsumption drops
        // the home duplicate) and the value is born co-located. A failed check is captured and raised
        // after the fold (the discarded re-home is harmless).
        let home = contract
            .expect("a declared return type implies a contract")
            .home_region();
        let home_carrier = Witnessed::<ContractHomeFamily, FrameSet>::new(
            (home, declared),
            carrier.witness().clone(),
        );
        let mut mismatch: Option<KError> = None;
        let restamped = carrier
            .merge::<ContractHomeFamily, CarriedFamily>(
                home_carrier,
                |value, (home_region, declared_type), _brand| {
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
                },
            )
            .expect("a FrameSet set witness always represents the union");
        match mismatch {
            Some(error) => Err(error),
            None => Ok(restamped),
        }
    }
}

/// Enforce a `Done` step's declared return contract, returning the slot's final terminal. A `None`
/// frame (a frameless slot or the non-dying run frame) passes the value through untouched. A failed
/// return-type check becomes `Err` — the caller clears placeholders and finalizes. A non-coarsening
/// check leaves the value in the producer frame; a coarsening re-tag is re-allocated into the
/// contract's own home region (`ReturnContract::home_region` — the callee's captured-scope / arm
/// call-site region, a strict ancestor of the producer frame) so the re-tagged terminal outlives the
/// reused/freed producer frame. Reads no scope: the home region rides the contract, witnessed by the
/// cart `Rc`.
fn enforce_return_contract<'o>(
    output: Result<Carried<'o>, KError>,
    frame: Option<&Rc<CallFrame>>,
    contract: Option<ReturnContract<'o>>,
) -> Result<Carried<'o>, KError> {
    match (output, frame) {
        (Ok(Carried::Object(v)), Some(_)) => {
            match check_declared_return(contract, |d| d.matches_value(v), || v.ktype().name())? {
                // Re-tag to the declared return type so downstream dispatch sees the contract
                // (may coarsen, e.g. `List<Number>` through `:(LIST OF Any)` -> `List<Any>`). The
                // re-tag is a shallow rebuild homed in the contract's own home region, since the
                // producer frame it was born in may be reused or freed before consumers read it.
                Some(declared) => {
                    let stamped = v.deep_clone().stamp_type(declared);
                    let home = contract
                        .expect("a declared return type implies a contract")
                        .home_region();
                    Ok(Carried::Object(home.alloc_object(stamped)))
                }
                None => Ok(Carried::Object(v)),
            }
        }
        // A type flowing the type channel runs the shared declared-return check via `matches_type`.
        // The type channel ignores the returned declared type — unlike the `Object` arm, it does
        // not re-tag — so the in-frame value passes through unchanged.
        (Ok(Carried::Type(t)), Some(_)) => {
            check_declared_return(contract, |d| d.matches_type(t), || t.name())?;
            Ok(Carried::Type(t))
        }
        (Err(e), Some(_frame)) => {
            let with_frame = match contract {
                Some(contract) => {
                    let label = match contract {
                        ReturnContract::Function(f) => f.summarize(),
                        ReturnContract::Arm { kind, .. } => kind.to_string(),
                        ReturnContract::PerCall { func, .. } => func.summarize(),
                    };
                    e.with_frame(crate::machine::TraceFrame::bare(label.clone(), label))
                }
                None => e,
            };
            Err(with_frame)
        }
        (other, None) => other,
    }
}

/// The declared-return check shared by the `Object` and `Type` finalize arms: pull the
/// declared return type off `contract` (a `Function`'s resolved `return_type`, or an
/// `Arm`'s `-> :T`), and if there is one, verify the lifted carrier satisfies it.
/// `satisfies` runs the channel-appropriate predicate (`matches_value` / `matches_type`)
/// and `got_name` names the carrier for the mismatch error. Returns the declared type so
/// the caller can re-tag against it (the `Object` arm coarsens; the `Type` arm discards
/// it), `Ok(None)` when nothing is declared — a `Function` whose signature return is
/// non-`Resolved` (a `Deferred` carrier still in its FN-def signature) has no type here —
/// or `Err` with the labelled `TypeMismatch`. A `PerCall` carries the *resolved* per-call
/// type and is checked + stamped here, labelled "per-call return type".
fn check_declared_return<'o>(
    contract: Option<ReturnContract<'o>>,
    satisfies: impl FnOnce(&KType<'o>) -> bool,
    got_name: impl FnOnce() -> String,
) -> Result<Option<&'o KType<'o>>, KError> {
    let Some((declared, label, per_call)) = pull_declared_return(contract) else {
        return Ok(None);
    };
    if !satisfies(declared) {
        return Err(return_type_mismatch(declared, per_call, &label, got_name()));
    }
    Ok(Some(declared))
}

/// Pull the declared return type off `contract` plus its diagnostic label and the `per_call` flag, or
/// `None` when nothing is declared — no contract, or a `Function` whose signature return is
/// non-`Resolved` (a `Deferred` carrier still in its FN-def signature). The extraction half of
/// [`check_declared_return`]; the witnessed path reuses it so the value check can run *inside* the
/// re-stamp `merge` (where the carrier value and the declared type meet at one brand).
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
