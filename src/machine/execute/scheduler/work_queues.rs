use std::collections::VecDeque;

/// Routing + priority wrapper around the scheduler's two work queues.
///
/// Routing: a fresh submission (no active frame, no deps) lands in `fresh`;
/// everything else lands in `in_flight`. Priority: `in_flight` drains ahead of
/// `fresh` so an in-progress computation finishes before the next fresh top-level
/// expression starts. Both rules are enforced by the method surface.
#[derive(Default)]
pub(super) struct WorkQueues {
    fresh: VecDeque<usize>,
    in_flight: VecDeque<usize>,
}

impl WorkQueues {
    pub(super) fn new() -> Self { Self::default() }

    pub(super) fn pop_next(&mut self) -> Option<usize> {
        self.in_flight.pop_front().or_else(|| self.fresh.pop_front())
    }

    pub(super) fn push_fresh(&mut self, idx: usize) {
        self.fresh.push_back(idx);
    }

    pub(super) fn push_in_flight_submit(&mut self, idx: usize) {
        self.in_flight.push_back(idx);
    }

    /// Front-loads onto the in-flight band so the tail step runs before any
    /// sibling work.
    pub(super) fn push_after_replace(&mut self, idx: usize) {
        self.in_flight.push_front(idx);
    }

    pub(super) fn push_woken(&mut self, idx: usize) {
        self.in_flight.push_back(idx);
    }
}
