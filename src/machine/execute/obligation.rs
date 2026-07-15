//! The declared-return obligation a tail chain carries as a continuation capture.
//!
//! The obligation travels as a koan continuation capture — the self-pinning sealed contract cell plus
//! its precomputed trace label. A continuation whose slot has a declared-return obligation is wrapped by
//! [`with_obligation`] before it is boxed, so the obligation deposits into the ambient slot-step
//! state at the top of every step and is visible to the readers, the Done-boundary check, and the
//! error-label path within the step's dynamic extent.

use crate::machine::core::{ReturnContract, SealedContract};
use crate::machine::FrameSet;
use crate::witnessed::{Erased, Sealed, Witnessed};

use super::outcome::NodeContinuation;

/// A slot's declared-return obligation, riding the tail chain as a continuation capture.
/// Self-pinning: `cell`'s `FrameSet` witness holds the contract's home-region owner, so the home
/// region stays live across every hop of a tail chain independent of which cart the slot carries.
/// `label` is the trace frame for the error path, precomputed at seal time so an errored step never
/// has to open the cell.
pub(in crate::machine::execute) struct ReturnObligation {
    cell: SealedContract,
    label: String,
}

impl ReturnObligation {
    /// Seal a live [`ReturnContract`] into its dormant, lifetime-free obligation form. The label is
    /// derived once here from the contract variant so the error path reads it back rather than
    /// re-deriving. The cell seals against the contract's own carried witness — its home owner's
    /// `Rc`, folded into a `FrameSet` singleton (a genuine pinning witness) — not the cart's `outer`
    /// chain, so the kept-first contract's home region stays pinned across every hop of a tail chain.
    pub(in crate::machine::execute) fn seal(contract: ReturnContract<'_>) -> Self {
        let label = match contract {
            ReturnContract::Function(f) => f.summarize(),
            ReturnContract::Arm { kind, .. } => kind.to_string(),
            ReturnContract::PerCall { func, .. } => func.summarize(),
        };
        let cell = Sealed::seal(Witnessed::from_erased(
            Erased::erase(contract),
            contract
                .home_owner()
                .map_or_else(FrameSet::empty, FrameSet::singleton),
        ));
        ReturnObligation { cell, label }
    }

    /// Duplicate the obligation cheaply — the erased contract is `Copy` and the `FrameSet` witness
    /// clones (routing [`Sealed::duplicate`]), the label clones. Keep-first and the deposit both hand
    /// copies around, so neither consumes the original.
    pub(in crate::machine::execute) fn duplicate(&self) -> Self {
        ReturnObligation {
            cell: self.cell.duplicate(),
            label: self.label.clone(),
        }
    }

    /// Open the sealed contract under its **own** `FrameSet` witness — self-pinning, so no external
    /// pin is needed: the cell's witness holds the contract's home-region owner. Re-anchors the
    /// dormant cell into a live [`ReturnContract`] at a rank-2 brand and hands it to `f`; the brand
    /// forbids the live contract escaping, so `f`'s result names no brand. The Done-boundary check
    /// runs entirely inside this open.
    pub(in crate::machine::execute) fn open_cell<R>(
        &self,
        f: impl FnOnce(ReturnContract<'_>) -> R,
    ) -> R {
        self.cell.open(f)
    }

    /// The precomputed trace label — read on the error path instead of re-deriving it off a live
    /// contract, and on the value path to label a declared-return mismatch.
    pub(in crate::machine::execute) fn label(&self) -> &str {
        &self.label
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
