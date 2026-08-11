//! `RunId` — process-global identity for one `KoanRuntime` run.
//!
//! `NodeId`s are scheduler-local and restart from zero on every runtime, so a bare
//! `NodeId` cannot tell a declaration statement in one run apart from a same-positioned
//! statement in a later run over the same persistent scope. Qualifying it with a `RunId`
//! restores cross-run identity. The counter is process-global (precedent: `ScopeId`'s
//! `idx` counter in [`scope_id`](super::scope_id)) rather than per-root, because the
//! per-root storage a koan run hangs off is a workgraph type and must not carry a koan
//! concern.
//!
//! An install no scheduler drove carries no `RunId` at all — it is
//! [`NodeHandle::OffScheduler`](super::NodeHandle), a variant rather than a reserved
//! counter value.

use std::sync::atomic::{AtomicU64, Ordering};

/// Identity of one [`KoanRuntime`](crate::machine::execute::KoanRuntime) run. Minted once
/// per runtime from a global counter.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RunId(pub u64);

impl RunId {
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
    }
}
