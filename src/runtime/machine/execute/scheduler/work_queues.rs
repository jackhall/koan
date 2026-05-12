use std::collections::VecDeque;

/// Routing + priority wrapper around the scheduler's two work queues. Replaces the
/// pair of raw `VecDeque<usize>` fields on `Scheduler<'a>`; every push and pop site
/// in `submit.rs` and `execute.rs` migrates to one of the five methods below.
///
/// Routing rule: a fresh submission (no active frame, no deps) lands in the `fresh`
/// band; everything else — internal submissions, woken consumers, Replace-arm
/// re-enqueues — lands in the `in_flight` band. Priority rule: `in_flight` drains
/// ahead of `fresh` so an in-progress computation finishes before the next fresh
/// top-level expression starts. Both rules are enforced by the method surface —
/// callers cannot pick the wrong arm.
#[derive(Default)]
pub(super) struct WorkQueues {
    /// Fresh band: submissions from outside any slot's run (no active frame, no
    /// deps), FIFO in submission order.
    fresh: VecDeque<usize>,
    /// In-flight band: work belonging to a computation that's already started —
    /// internal submissions, woken consumers, Replace-arm re-enqueues. Drained
    /// ahead of `fresh` so users see sequential evaluation between fresh top-level
    /// statements.
    in_flight: VecDeque<usize>,
}

impl WorkQueues {
    pub(super) fn new() -> Self { Self::default() }

    /// Drain priority: in-flight band first, then fresh band. Returns `None` when
    /// both are empty — the scheduler's main loop treats that as "done."
    pub(super) fn pop_next(&mut self) -> Option<usize> {
        self.in_flight.pop_front().or_else(|| self.fresh.pop_front())
    }

    /// Enqueue a fresh submission (no active frame, no pending deps).
    pub(super) fn push_fresh(&mut self, idx: usize) {
        self.fresh.push_back(idx);
    }

    /// Enqueue an in-flight slot submitted with already-terminal deps.
    pub(super) fn push_in_flight_submit(&mut self, idx: usize) {
        self.in_flight.push_back(idx);
    }

    /// Re-enqueue a slot rewritten by the Replace-arm tail-call path. Front-loads
    /// it onto the in-flight band so the tail step runs before any sibling work.
    pub(super) fn push_after_replace(&mut self, idx: usize) {
        self.in_flight.push_front(idx);
    }

    /// Enqueue a consumer woken by a producer's terminal write.
    pub(super) fn push_woken(&mut self, idx: usize) {
        self.in_flight.push_back(idx);
    }
}
