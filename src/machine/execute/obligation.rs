//! The declared-return obligation a tail chain carries as a continuation capture.
//!
//! The obligation travels as a koan continuation capture — pure `Copy` data: the declared return
//! type (with its per-call flag) resolved once at seal time, plus the precomputed trace label. A
//! continuation whose slot has a declared-return obligation is wrapped by [`with_obligation`] before
//! it is boxed, so the obligation deposits into the ambient slot-step state at the top of every step
//! and is visible to the readers, the Done-boundary check, and the error-label path within the
//! step's dynamic extent.

use crate::machine::core::ReturnContract;
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
    /// Seal a live [`ReturnContract`] into its dormant, lifetime-free obligation form. Both the label
    /// and the declared return are resolved once here off the contract variant — the declared type
    /// is a `Copy` handle read directly (an FN reads its signature return, an arm/per-call its
    /// borrowed `ret`), so the obligation stores no reference into the contract's region.
    pub(in crate::machine::execute) fn seal(contract: ReturnContract<'_>) -> Self {
        let label = match contract {
            ReturnContract::Function(f) => f.summarize(),
            ReturnContract::Arm { kind, .. } => kind.to_string(),
            ReturnContract::PerCall { func, .. } => func.summarize(),
        };
        ReturnObligation {
            declared: pull_declared_return(contract),
            label,
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

/// Pull the declared return type off `contract` plus its `per_call` flag, or `None` when nothing is
/// declared — a `Function` whose signature return is non-`Resolved` (a `Deferred` carrier still in
/// its FN-def signature). The diagnostic label rides the [`ReturnObligation`] separately.
fn pull_declared_return(contract: ReturnContract<'_>) -> Option<(KType, bool)> {
    match contract {
        ReturnContract::Function(f) => match &f.signature.return_type {
            ReturnType::Resolved(d) => Some((*d, false)),
            _ => None,
        },
        ReturnContract::Arm { ret, .. } => Some((ret, false)),
        ReturnContract::PerCall { ret, .. } => Some((ret, true)),
    }
}

/// Wrap a live continuation so it deposits `obligation` into the ambient slot-step state before
/// running. Applied to the outermost closure at the point where the live [`NodeContinuation`] is
/// boxed, before `NodeWork::new` erases it — the whole invariant that carries the declared-return
/// checker down a tail chain.
pub(in crate::machine::execute) fn with_obligation<'a>(
    obligation: ReturnObligation,
    inner: NodeContinuation<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, deps, idx| {
        view.deposit_obligation(obligation);
        inner(view, deps, idx)
    })
}
