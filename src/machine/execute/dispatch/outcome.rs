//! The dispatch-side effect currency — the peer of
//! [`Action`](crate::machine::core::kfunction::action::Action) for the dispatcher.
//!
//! A dispatch *shape handler* decides against a read-only view of the scheduler and
//! **returns** its scheduler mutations as a [`DispatchOutcome`]; the harness
//! ([`super::harness`]) interprets that outcome and is the sole place that writes the
//! scheduler graph. This is the contract that lets `Scheduler` become the only
//! `SchedulerHandle` impl — a handler never reaches `&mut Scheduler` directly.
//!
//! The enum grows one variant per migrated handler (so an un-migrated handler that still
//! returns `NodeStep` produces no dead arm); the end state is a closed set the harness
//! interprets exhaustively.

use super::super::nodes::NodeOutput;

/// What a decided dispatch slot wants the harness to do. Each variant is a pure data
/// description of a scheduler effect — no `&mut Scheduler` is captured.
pub(in crate::machine::execute) enum DispatchOutcome<'run> {
    /// Complete this slot with a value or error — the dispatcher reached a terminal with no
    /// graph write. The harness lowers it straight to [`NodeStep::Done`](super::super::nodes::NodeStep).
    Terminal(NodeOutput<'run>),
}
