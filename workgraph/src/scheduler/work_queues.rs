use std::collections::VecDeque;

use super::NodeId;

/// Routing + priority wrapper around the scheduler's two work queues.
///
/// Routing: a fresh submission (no active frame, no deps) lands in `fresh`;
/// everything else lands in `in_flight`. Priority: `in_flight` drains ahead of
/// `fresh` so an in-progress computation finishes before the next fresh top-level
/// expression starts. Both rules are enforced by the method surface.
#[derive(Default)]
pub(in crate::scheduler) struct WorkQueues {
    fresh: VecDeque<NodeId>,
    in_flight: VecDeque<NodeId>,
}

impl WorkQueues {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn pop_next(&mut self) -> Option<NodeId> {
        self.in_flight
            .pop_front()
            .or_else(|| self.fresh.pop_front())
    }

    pub(super) fn push_fresh(&mut self, id: NodeId) {
        self.fresh.push_back(id);
    }

    pub(super) fn push_in_flight_submit(&mut self, id: NodeId) {
        self.in_flight.push_back(id);
    }

    /// Tail step runs before any sibling work.
    pub(super) fn push_after_replace(&mut self, id: NodeId) {
        self.in_flight.push_front(id);
    }

    pub(super) fn push_woken(&mut self, id: NodeId) {
        self.in_flight.push_back(id);
    }
}
