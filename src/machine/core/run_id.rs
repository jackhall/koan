//! `RunId` — process-global identity for one `KoanRuntime` run.
//!
//! `NodeId`s are scheduler-local and restart from zero on every runtime, so a bare
//! `NodeId` cannot tell a declaration statement in one run apart from a same-positioned
//! statement in a later run over the same persistent scope. Qualifying it with a `RunId`
//! restores cross-run identity. The counter is process-global (precedent: `ScopeId`'s
//! `idx` counter in [`scope_id`](super::scope_id)) rather than per-root, because the
//! per-root storage a koan run hangs off is a workgraph type and must not carry a koan
//! concern.

use std::sync::atomic::{AtomicU64, Ordering};

/// Identity of one [`KoanRuntime`](crate::machine::execute::KoanRuntime) run. Minted once
/// per runtime from a global counter. [`RunId::OFF_SCHEDULER`] (`RunId(0)`) is reserved for
/// off-scheduler registration (builtins); minted ids start at 1.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RunId(pub u64);

impl RunId {
    /// The run of an off-scheduler registration — builtin type installs that no scheduler
    /// slot drove. Minted ids start at 1, so this cannot collide with a real run.
    pub const OFF_SCHEDULER: RunId = RunId(0);

    pub fn next() -> RunId {
        RunId(RUN_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_ids_are_distinct() {
        let a = RunId::next();
        let b = RunId::next();
        assert_ne!(a, b);
        assert_ne!(a, RunId::OFF_SCHEDULER);
        assert_ne!(b, RunId::OFF_SCHEDULER);
    }
}
