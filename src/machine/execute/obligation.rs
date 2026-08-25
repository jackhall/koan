//! The declared-return obligation a tail chain carries as a continuation capture.
//!
//! Sealing resolves the declared return type and retains the handles an error frame renders from,
//! leaving pure `Copy` data — the chain carries nothing region-bound, no path downstream reopens a
//! live contract, and a call that returns cleanly renders no trace text at all.
//! [`with_obligation`] wraps a continuation so the obligation deposits into the ambient slot-step
//! state at the top of every step, visible to the readers, the Done-boundary check, and the
//! error-frame path for that step's dynamic extent.

use crate::machine::core::{KFunction, ReturnContract};
use crate::machine::model::{KType, ReturnType};

use super::outcome::{DeferredTraceFrame, NodeContinuation};

/// A slot's declared-return obligation, riding the tail chain as a continuation capture. It carries
/// no pin: it is a checker and a retained frame, not a lifetime — the value it checks escapes only
/// at the bind seam, staying resident in its producer region.
#[derive(Clone, Copy)]
pub(in crate::machine::execute) struct ReturnObligation {
    declared: Option<(KType, bool)>,
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
                    frame: DeferredTraceFrame::Callable {
                        site,
                        ktype: f.value_ktype(),
                    },
                })
            }
            ReturnContract::PerCall { func, ret, site } => {
                func.open(|f: &KFunction<'_>| ReturnObligation {
                    declared: Some((ret, true)),
                    frame: DeferredTraceFrame::Callable {
                        site,
                        ktype: f.value_ktype(),
                    },
                })
            }
        }
    }

    /// The declared return type and its per-call flag, `None` when nothing is declared.
    pub(in crate::machine::execute) fn declared(&self) -> Option<(KType, bool)> {
        self.declared
    }

    /// The retained error frame, rendered only by the error arms that spend it.
    pub(in crate::machine::execute) fn frame(&self) -> DeferredTraceFrame<'static> {
        self.frame
    }
}

/// Wrap a live continuation so it deposits `obligation` into the ambient slot-step state before
/// running. A `None` obligation passes `inner` through unchanged, so construction sites state the
/// fold once instead of each matching on the option.
pub(in crate::machine::execute) fn with_obligation<'a>(
    obligation: Option<ReturnObligation>,
    inner: NodeContinuation<'a>,
) -> NodeContinuation<'a> {
    let Some(obligation) = obligation else {
        return inner;
    };
    Box::new(move |view, deps, idx| {
        view.deposit_obligation(obligation);
        inner(view, deps, idx)
    })
}
