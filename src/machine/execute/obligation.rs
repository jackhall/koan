//! The declared-return obligation a tail chain carries as a continuation capture.
//!
//! Sealing resolves the declared return type and retains the handles an error frame renders from,
//! leaving pure `Copy` data — the chain carries nothing region-bound, no path downstream reopens a
//! live contract, and a call that returns cleanly renders no trace text at all.
//! The obligation rides beside the continuation as plain data, inside the [`ParkState`] the slot
//! stores; the step deposits that state into the ambient slot-step context before running the
//! closure, so the obligation is visible to the readers, the Done-boundary check, and the
//! error-frame path for that step's dynamic extent.

use std::rc::Rc;

use crate::machine::CallFrame;
use crate::machine::core::{KFunction, ReturnContract};
use crate::machine::model::{KType, MemberCoercion, ReturnType};

use super::outcome::DeferredTraceFrame;

/// The ambient slot-step state a park carries across the slot's dormancy: the declared-return
/// obligation the chain established, plus the block frame a leading-carrying tail keeps alive for
/// its finish. It rides the stored
/// [`NodeContinuation`](super::outcome::NodeContinuation) as plain data, and the step re-deposits
/// it into the ambient context before running the call, so both are ambient reads for that step's
/// dynamic extent.
///
/// Holding the frame here rather than in the finish's captures is what keeps that closure's capture
/// set `Copy`: a leading-carrying tail erases onto the bumped tier and allocates nothing.
#[derive(Default, Clone)]
pub(in crate::machine::execute) struct ParkState {
    pub(in crate::machine::execute) obligation: Option<ReturnObligation>,
    pub(in crate::machine::execute) block_frame: Option<Rc<CallFrame>>,
}

/// A slot's declared-return obligation, riding the tail chain as a continuation capture. It carries
/// no pin: it is a checker and a retained frame, not a lifetime — the value it checks escapes only
/// at the bind seam, staying resident in its producer region.
#[derive(Clone, Copy)]
pub(in crate::machine::execute) struct ReturnObligation {
    declared: Option<(KType, bool)>,
    /// The ascription-barrier rewrite a coerced call's result takes once it has satisfied
    /// `declared`: the SIG-declared return type the walk recurses on, and the two member bindings
    /// it rewrites between. `None` for every ordinary return. Both halves are `Copy` and
    /// lifetime-free — a [`MemberCoercion`] carries its tables as interned handles — so the
    /// obligation still names no region.
    coercion: Option<(KType, MemberCoercion)>,
    frame: DeferredTraceFrame<'static>,
}

impl ReturnObligation {
    /// Seal a [`ReturnContract`] into its dormant, lifetime-free obligation form. A callable
    /// contract is opened here, under the seal's own `'home` brand, so no pin is named at all —
    /// the open reads the `Copy` `value_ktype` handle and keeps nothing borrowed.
    pub(in crate::machine::execute) fn seal(contract: ReturnContract<'_>) -> Self {
        match contract {
            ReturnContract::Arm { ret, kind } => ReturnObligation {
                declared: Some((ret, false)),
                coercion: None,
                frame: DeferredTraceFrame::Bare {
                    function: kind,
                    expression: kind,
                },
            },
            // A `Function`'s declared return is absent while its signature's return is still a
            // `Deferred` carrier.
            ReturnContract::Function { func, site } => {
                func.open(|f: &KFunction<'_>| ReturnObligation {
                    declared: match f.signature.return_type() {
                        ReturnType::Resolved(d) => Some((d, false)),
                        _ => None,
                    },
                    coercion: None,
                    frame: DeferredTraceFrame::Callable {
                        site,
                        ktype: f.value_ktype(),
                    },
                })
            }
            ReturnContract::PerCall { func, ret, site } => {
                func.open(|f: &KFunction<'_>| ReturnObligation {
                    declared: Some((ret, true)),
                    coercion: None,
                    frame: DeferredTraceFrame::Callable {
                        site,
                        ktype: f.value_ktype(),
                    },
                })
            }
            ReturnContract::Coerced {
                func,
                expected,
                declared,
                coercion,
                site,
            } => func.open(|f: &KFunction<'_>| ReturnObligation {
                declared: Some((expected, false)),
                coercion: Some((declared, coercion)),
                frame: DeferredTraceFrame::Callable {
                    site,
                    ktype: f.value_ktype(),
                },
            }),
        }
    }

    /// The declared return type and its per-call flag, `None` when nothing is declared.
    pub(in crate::machine::execute) fn declared(&self) -> Option<(KType, bool)> {
        self.declared
    }

    /// The ascription-barrier rewrite this return takes after its declared check, if any.
    pub(in crate::machine::execute) fn coercion(&self) -> Option<(KType, MemberCoercion)> {
        self.coercion
    }

    /// The retained error frame, rendered only by the error arms that spend it.
    pub(in crate::machine::execute) fn frame(&self) -> DeferredTraceFrame<'static> {
        self.frame
    }
}
