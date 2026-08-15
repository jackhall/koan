//! `StatementId` — identity of one submitted statement, minted by koan alone.
//!
//! A binding table has to tell one declaration statement re-entering (a parallel nominal
//! finalize, which must overwrite idempotently) from a second declaration of the same name
//! (which must `Rebind`). Lexical position cannot decide it: a detached chain gives every
//! submission index `0`, so two textually identical declarations name no position to tell
//! them apart. Content cannot decide it either — a byte-identical redeclaration is still a
//! redeclaration.
//!
//! What distinguishes them is only *that they are separate submissions*, so that is what
//! this names. The counter is process-global (precedent: `ScopeId`'s `idx` counter in
//! [`scope_id`](super::scope_id)) and never recycled, so an id stays unique for as long as
//! the entry holding it lives — which outlasts the slot that installed it, and outlasts the
//! run. A scheduler handle could not carry that: slot and edge indices are recycled, so a
//! later declaration can be handed a freed index and compare equal to the entry it should
//! be rebinding. Minting koan's own id is also what keeps the machine core free of
//! scheduler currency — the binding tables name a *statement*, a koan concept, and the DAG
//! layer owes them nothing.
//!
//! An install no statement drove carries no id at all — it is
//! [`Installer::NoStatement`](super::Installer), a variant rather than a reserved counter
//! value.

use std::sync::atomic::{AtomicU64, Ordering};

/// Identity of one submitted statement. Minted per slot anchor and carried across a tail
/// replace, so a slot keeps one id for its whole life however many times it steps.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct StatementId(pub u64);

impl StatementId {
    pub fn next() -> StatementId {
        StatementId(STATEMENT_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

static STATEMENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_ids_are_distinct() {
        let a = StatementId::next();
        let b = StatementId::next();
        assert_ne!(a, b);
    }
}
