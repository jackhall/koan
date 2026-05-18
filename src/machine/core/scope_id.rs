//! `ScopeId` — position-independent identity for `Scope` instances.
//!
//! Replaces the prior `*const Scope as usize` identity scheme. Pointer-derived
//! identity couples equality to memory placement, so a relocated or freed scope
//! would silently break dispatch on user-declared types. A counter-allocated
//! newtype decouples identity from the pointer.
//!
//! Layout is `(session: u64, idx: u64)`. `session` is minted once per process from
//! `RandomState`-derived entropy; `idx` is from a global `AtomicU64` counter. The
//! pair gives:
//! - Within a session: monotonic, never-aliasing identity.
//! - Across sessions: cross-process collision probability of 2⁻⁶⁴ — sufficient
//!   for non-adversarial use such as the planned compile-then-run split, where
//!   one process serializes a scope graph and another loads and runs it.
//!
//! Merging multiple serialized snapshots in one process would also Just Work
//! under this scheme: session halves differ, so loaded ids cannot collide. Single-
//! snapshot loads can also seed the local counter to `max(loaded.idx) + 1` if a
//! matching session is detected — not implemented today.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Position-independent identity for a [`Scope`](super::Scope). Equality is by
/// the `(session, idx)` pair — minted once per scope at construction time, no
/// pointer-derived state.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId {
    session: u64,
    idx: u64,
}

impl ScopeId {
    /// Sentinel id reserved for the `_typeconstructor` placeholder in
    /// [`crate::builtins::type_ops`] — a parametric carrier with no
    /// concrete declaring scope. Session and idx are both 0; real minted ids
    /// have a nonzero random session, so the sentinel cannot collide.
    pub const SENTINEL: ScopeId = ScopeId { session: 0, idx: 0 };

    /// Mint a fresh `ScopeId`. Called by [`Scope`](super::Scope) constructors.
    pub fn next() -> ScopeId {
        ScopeId { session: session_id(), idx: next_idx() }
    }

    /// Build a `ScopeId` from raw halves. Reserved for:
    /// - Test fixtures constructing identity-equal pairs.
    /// - Future deserialization paths (each loaded scope retains its original
    ///   `(session, idx)`; the local counter is seeded past the loaded max).
    ///
    /// Production code outside those paths should use [`Self::next`] or
    /// [`Self::SENTINEL`].
    pub const fn from_raw(session: u64, idx: u64) -> ScopeId {
        ScopeId { session, idx }
    }

    pub const fn session(self) -> u64 { self.session }
    pub const fn idx(self) -> u64 { self.idx }
}

impl std::fmt::Debug for ScopeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ScopeId({:x}:{:x})", self.session, self.idx)
    }
}

impl std::fmt::Display for ScopeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:x}", self.idx)
    }
}

/// Process-wide `session` half. Minted once on first call; reused thereafter.
/// `RandomState::new()` pulls process-start entropy on every supported platform,
/// so this is OS-RNG-seeded without taking a `getrandom`/`rand` dependency.
fn session_id() -> u64 {
    static SESSION: OnceLock<u64> = OnceLock::new();
    *SESSION.get_or_init(|| {
        let mut h = RandomState::new().build_hasher();
        h.write_u8(0);
        let v = h.finish();
        if v == 0 { 1 } else { v }
    })
}

static IDX_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_idx() -> u64 {
    IDX_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_ids_are_distinct() {
        let a = ScopeId::next();
        let b = ScopeId::next();
        assert_ne!(a, b);
        assert_eq!(a.session(), b.session());
        assert_ne!(a.idx(), b.idx());
    }

    #[test]
    fn sentinel_cannot_collide_with_minted() {
        let live = ScopeId::next();
        assert_ne!(ScopeId::SENTINEL, live);
        assert_eq!(ScopeId::SENTINEL.idx(), 0);
        assert_eq!(ScopeId::SENTINEL.session(), 0);
    }

    #[test]
    fn from_raw_with_zero_session_disjoint_from_minted() {
        let live = ScopeId::next();
        let fake = ScopeId::from_raw(0, 0xAA);
        assert_ne!(live, fake);
    }
}
