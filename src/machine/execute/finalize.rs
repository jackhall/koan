use std::rc::Rc;

use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::model::{Carried, KType};
use crate::machine::{CallFrame, KError, KErrorKind};

use super::runtime::KoanRuntime;

/// The workload's Done-boundary contract hook: enforce a finished node's declared return contract,
/// returning the slot's final terminal. The driver vends the slot's contract already re-anchored
/// (the scheduler owns that reattach in `vend_carrier`) and hands this hook the live
/// [`ReturnContract`] plus the (optional) per-call frame; the hook runs the declared-return check.
/// The scheduler decides *when* (the Done boundary); this hook owns the `ReturnContract`-/
/// `KType`-naming *how*, so the scheduler core names neither.
///
/// Peer of [`NodeLift`](super::lift::NodeLift): both are workload hooks the Done boundary calls. Lift
/// relocates a value across a dep edge; finalize enforces the return contract. They stay separate —
/// the contract layer is never folded into the lift (see [`lift`](super::lift)).
///
/// A Koan-typed workload hook: the generic scheduler ([`crate::scheduler`]) drives the Done
/// boundary through this trait (alongside `NodeLift`) and names no Koan type itself.
///
/// Single-lifetime (`'o -> 'o`): the value arrives already at its destination node lifetime `'o`
/// (the step lifetime the producer ran against), so the hook re-anchors only the contract — never
/// the value — and the coarsening re-tag homes into the contract's own arena at the same `'o`.
pub(in crate::machine::execute) trait NodeFinalize {
    /// Enforce the declared return on `output` against the already-vended live `contract`. A `None`
    /// frame (a frameless slot or the non-dying run frame) passes the value through untouched — and
    /// the driver vends no contract for such a producer, so `contract` is `None` there too.
    fn finalize_terminal<'o>(
        &self,
        output: Result<Carried<'o>, KError>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Carried<'o>, KError>;
}

impl NodeFinalize for KoanRuntime<'_> {
    fn finalize_terminal<'o>(
        &self,
        output: Result<Carried<'o>, KError>,
        frame: Option<&Rc<CallFrame>>,
        contract: Option<ReturnContract<'o>>,
    ) -> Result<Carried<'o>, KError> {
        enforce_return_contract(output, frame, contract)
    }
}

/// Enforce a `Done` step's declared return contract, returning the slot's final terminal. A `None`
/// frame (a frameless slot or the non-dying run frame) passes the value through untouched. A failed
/// return-type check becomes `Err` — the caller clears placeholders and finalizes. A non-coarsening
/// check leaves the value in the producer frame; a coarsening re-tag is re-allocated into the
/// contract's own home arena (`ReturnContract::home_arena` — the callee's captured-scope / arm
/// call-site arena, a strict ancestor of the producer frame) so the re-tagged terminal outlives the
/// reused/freed producer frame. Reads no scope: the home arena rides the contract, witnessed by the
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
                // re-tag is a shallow rebuild homed in the contract's own home arena, since the
                // producer frame it was born in may be reused or freed before consumers read it.
                Some(declared) => {
                    let stamped = v.deep_clone().stamp_type(declared);
                    let home = contract
                        .expect("a declared return type implies a contract")
                        .home_arena();
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
    let (declared, label, per_call) = match contract {
        Some(ReturnContract::Function(f)) => match &f.signature.return_type {
            crate::machine::model::types::ReturnType::Resolved(d) => (d, f.summarize(), false),
            _ => return Ok(None),
        },
        Some(ReturnContract::Arm { ret, kind, .. }) => (ret, kind.to_string(), false),
        Some(ReturnContract::PerCall { func, ret }) => (ret, func.summarize(), true),
        None => return Ok(None),
    };
    if !satisfies(declared) {
        let expected = if per_call {
            format!("{} (per-call return type)", declared.name())
        } else {
            declared.name()
        };
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "<return>".to_string(),
            expected,
            got: got_name(),
        })
        .with_frame(crate::machine::TraceFrame::bare(label.clone(), label)));
    }
    Ok(Some(declared))
}
