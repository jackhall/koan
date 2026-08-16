//! The declared-return obligation a tail chain carries as a continuation capture.
//!
//! The obligation travels as a koan continuation capture — pure `Copy` data: the declared return
//! type (with its per-call flag) resolved once at seal time, plus the precomputed trace label. A
//! continuation whose slot has a declared-return obligation is wrapped by [`with_obligation`] before
//! it is boxed, so the obligation deposits into the ambient slot-step state at the top of every step
//! and is visible to the readers, the Done-boundary check, and the error-label path within the
//! step's dynamic extent.

use crate::machine::core::{KFunction, ReturnContract};
use crate::machine::model::{KType, ReturnType};

use super::outcome::NodeContinuation;

/// A slot's declared-return obligation, riding the tail chain as a continuation capture. Pure data:
/// `declared` is the return type and per-call flag resolved off the contract at seal time (a `Copy`
/// [`KType`] handle needs no home-region pin), `None` when nothing is declared. `label` is the trace
/// frame for the error path, precomputed at seal time so no path reopens a live contract. The value
/// escapes only at the bind seam, staying resident in its producer region, so the obligation carries
/// no pin: it is a checker and a label, not a lifetime.
pub(in crate::machine::execute) struct ReturnObligation {
    declared: Option<(KType, bool)>,
    label: String,
}

impl ReturnObligation {
    /// Seal a [`ReturnContract`] into its dormant, lifetime-free obligation form. Both the label and
    /// the declared return are resolved once here, so nothing downstream reopens a contract: an arm
    /// reads its `Copy` handle directly, and a callable contract re-opens its seal under the seal's
    /// own `'home` brand, so no pin is named here at all. What the obligation keeps is a `Copy`
    /// handle and an owned string, so the chain carries nothing region-bound.
    pub(in crate::machine::execute) fn seal(contract: ReturnContract<'_>) -> Self {
        match contract {
            ReturnContract::Arm { ret, kind } => ReturnObligation {
                declared: Some((ret, false)),
                label: kind.to_string(),
            },
            // A `Function`'s declared return is its signature's, and is absent when that return is
            // still a `Deferred` carrier in the FN-def signature.
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

    /// Duplicate the obligation — `declared` is `Copy`, the label clones. Keep-first and the deposit
    /// both hand copies around, so neither consumes the original.
    pub(in crate::machine::execute) fn duplicate(&self) -> Self {
        ReturnObligation {
            declared: self.declared,
            label: self.label.clone(),
        }
    }

    /// The declared return type and its per-call flag, resolved at seal time — `None` when nothing is
    /// declared. Read on the value path to check (and, for a non-union object, re-stamp) the produced
    /// value against the contract.
    pub(in crate::machine::execute) fn declared(&self) -> Option<(KType, bool)> {
        self.declared
    }

    /// The precomputed trace label — read on the error path to label the callee's trace frame, and on
    /// the value path to label a declared-return mismatch.
    pub(in crate::machine::execute) fn label(&self) -> &str {
        &self.label
    }
}

/// Wrap a live continuation so it deposits `obligation` into the ambient slot-step state before
/// running — a no-op pass-through on `None`, so every construction site states the fold once
/// instead of matching on the option itself. Applied to the outermost closure at the point where
/// the live [`NodeContinuation`] is boxed, before `NodeWork::new` erases it — the whole invariant
/// that carries the declared-return checker down a tail chain.
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
