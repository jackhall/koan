//! What a cell carries between its steps, for every kind of cell.
//!
//! A slab cell, a tree cell and a tenant differ in where their bytes live and in what holds them
//! in their table; they park identically. Each carries the same four slots, and the rules that
//! read them — what names a scratch bump, what a death clears, what comes off at a step's entry
//! and back at its end — are the same rules. They live here once, so the pools hand this struct
//! out rather than restating them, and the graph's kind dispatch is two accessors rather than one
//! per rule.

use crate::reattach::{Erased, ErasedOverBoth, Reattachable, ReattachableOverBoth};
use crate::receipt::{Delivery, ReceiptRun};

/// One cell's step slots: one slot per habitat for what it parks, its receipt run, and a
/// registration waiting to be laid down.
pub(crate) struct StepSlots<
    'graph,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
> {
    /// The continuation at rest, erased. It carries no reach of its own: every reference it can
    /// capture is at the executing cell's brand, `'here`, which names storage the cell's hold set
    /// already covers — its own region, or a region a pinned crossing minted in.
    continuation: Option<Erased<'graph, C>>,
    /// The scratch state at rest: what names the cell's scratch bump across a park, and so one of
    /// the two things that hold off the bump's reset at a step's end. Its family is over both
    /// brands, so the form holds storage at `'here` beside scratch at `'scratch`. Empty at birth,
    /// and cleared when the cell's death is declared — a dead cell is never entered.
    scratch_state: Option<ErasedOverBoth<'graph, S>>,
    /// The receipt run at rest, over the same bump — the other thing that names it. Cleared with
    /// the scratch state at the cell's death, and holding nothing alive: its slots carry
    /// `Dormant`s, which carry no reach, and erased values, which are bytes.
    receipts: Option<Erased<'graph, ReceiptRun<D>>>,
    /// A slot count a step registered, waiting for that step's end to lay it down. It names no
    /// byte, so it holds no reset off; the lay-down runs after the reset, which is what puts a
    /// re-registering cell's next run at the foot of a fresh bump.
    pending_receipts: Option<usize>,
}

impl<'graph, C: Reattachable<'graph>, S: ReattachableOverBoth<'graph>, D: Delivery<'graph>>
    StepSlots<'graph, C, S, D>
{
    /// The slots of a cell just born: whatever it was handed, and nothing else. A cell is born
    /// with no scratch state, no run and no registration — its first step is what puts any of them
    /// there.
    pub(crate) fn born(continuation: Option<Erased<'graph, C>>) -> Self {
        StepSlots {
            continuation,
            scratch_state: None,
            receipts: None,
            pending_receipts: None,
        }
    }

    /// Whether anything at rest here names the scratch bump the cell writes: its scratch state, or
    /// its receipt run. **One definition, three readings** — a step's entry, a step's end, and a
    /// tenant's departure — so the count a host keeps moves by the same rule it is read by.
    ///
    /// A pending registration is deliberately not one of them. It names no byte until the step
    /// end that lays it down, and that lay-down happens *after* the reset, which is what puts a
    /// re-registering cell's next run at the foot of a bump handed back whole.
    pub(crate) fn names_scratch(&self) -> bool {
        self.scratch_state.is_some() || self.receipts.is_some()
    }

    /// The receipt run at rest, erased.
    pub(crate) fn receipts(&self) -> Option<Erased<'graph, ReceiptRun<D>>> {
        self.receipts
    }

    pub(crate) fn set_receipts(&mut self, run: Option<Erased<'graph, ReceiptRun<D>>>) {
        self.receipts = run;
    }

    /// The slot count waiting to be laid down, taken. Whatever is pending, not only what the step
    /// that just ended registered: a step that panicked left its registration here, and a step end
    /// is where it is honoured.
    pub(crate) fn take_pending_receipts(&mut self) -> Option<usize> {
        self.pending_receipts.take()
    }

    pub(crate) fn set_pending_receipts(&mut self, count: usize) {
        self.pending_receipts = Some(count);
    }

    /// Both parked slots, off the cell for the length of its step. Each has its own family — the
    /// continuation's is over one region lifetime and re-anchors at the executing cell's `'here`,
    /// the scratch state's is over both and re-anchors at `'here` and `'scratch` together — so
    /// neither slot can hold the other's form.
    pub(crate) fn take_parked(
        &mut self,
    ) -> (Option<Erased<'graph, C>>, Option<ErasedOverBoth<'graph, S>>) {
        (self.continuation.take(), self.scratch_state.take())
    }

    /// Both slots back at rest, as the step left them.
    pub(crate) fn put_parked(
        &mut self,
        (continuation, scratch): (Option<Erased<'graph, C>>, Option<ErasedOverBoth<'graph, S>>),
    ) {
        self.continuation = continuation;
        self.scratch_state = scratch;
    }

    /// Drop everything that names the cell's scratch bump, at the cell's death. A dead cell is
    /// never entered, so nothing will read what it parked there again, and the bump stops waiting
    /// on a cell that will never read it. The continuation stays: it is not what holds the reset
    /// off, and the disposal is what takes it.
    pub(crate) fn clear_scratch(&mut self) {
        self.scratch_state = None;
        self.receipts = None;
        self.pending_receipts = None;
    }
}
