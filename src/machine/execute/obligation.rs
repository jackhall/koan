//! The declared-return obligation a tail chain carries as a continuation capture.
//!
//! Sealing resolves the declared return type and the trace label once, leaving pure `Copy` data
//! plus an owned string — the chain carries nothing region-bound, and no path downstream reopens a
//! live contract. [`with_obligation`] wraps a continuation so the obligation deposits into the
//! ambient slot-step state at the top of every step, visible to the readers, the Done-boundary
//! check, and the error-label path for that step's dynamic extent.

use crate::machine::core::{KFunction, ReturnContract};
use crate::machine::model::{KType, ReturnType};

use super::outcome::NodeContinuation;

/// A slot's declared-return obligation, riding the tail chain as a continuation capture. It carries
/// no pin: it is a checker and a label, not a lifetime — the value it checks escapes only at the
/// bind seam, staying resident in its producer region.
pub(in crate::machine::execute) struct ReturnObligation {
    declared: Option<(KType, bool)>,
    label: String,
}

impl ReturnObligation {
    /// Seal a [`ReturnContract`] into its dormant, lifetime-free obligation form. A callable
    /// contract is opened here, under the seal's own `'home` brand, so no pin is named at all.
    pub(in crate::machine::execute) fn seal(contract: ReturnContract<'_>) -> Self {
        match contract {
            ReturnContract::Arm { ret, kind } => ReturnObligation {
                declared: Some((ret, false)),
                label: kind.to_string(),
            },
            // A `Function`'s declared return is absent while its signature's return is still a
            // `Deferred` carrier.
            ReturnContract::Function(func) => func.open(|f: &KFunction<'_>| ReturnObligation {
                declared: match f.signature.return_type() {
                    ReturnType::Resolved(d) => Some((d, false)),
                    _ => None,
                },
                label: f.summarize(),
            }),
            ReturnContract::PerCall { func, ret } => {
                func.open(|f: &KFunction<'_>| ReturnObligation {
                    declared: Some((ret, true)),
                    label: f.summarize(),
                })
            }
        }
    }

    pub(in crate::machine::execute) fn duplicate(&self) -> Self {
        ReturnObligation {
            declared: self.declared,
            label: self.label.clone(),
        }
    }

    /// The declared return type and its per-call flag, `None` when nothing is declared.
    pub(in crate::machine::execute) fn declared(&self) -> Option<(KType, bool)> {
        self.declared
    }

    /// The precomputed trace label.
    pub(in crate::machine::execute) fn label(&self) -> &str {
        &self.label
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
