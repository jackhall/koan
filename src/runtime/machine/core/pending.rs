//! The deferred-write queue façade. `bind_value` / `register_function` route writes that
//! hit a `try_borrow_mut` collision (caller up the stack iterates `data` / `functions`)
//! through here; the scheduler drains the queue between dispatch nodes via
//! [`PendingQueue::drain`], replaying retries through the same validated [`Bindings`]
//! write path as direct writes so the dual-map invariant extends to drained writes by
//! construction.
//!
//! `PendingWrite` is module-private: adding a new write kind is a one-file change
//! (variant + `defer_*` constructor + `drain` match arm), no longer threads through
//! `Scope`.

use std::cell::RefCell;

use crate::runtime::machine::kfunction::KFunction;
use crate::runtime::machine::model::values::KObject;

use super::bindings::{ApplyOutcome, Bindings};

/// A pending re-entrant write — queued when `try_borrow_mut` on `data`/`functions` collides
/// with a borrow held up the call stack, retried by [`PendingQueue::drain`] between
/// scheduler nodes. The variant tag preserves the per-signature dedupe and value/function
/// collision check on retry, which a single shared retry path would skip.
enum PendingWrite<'a> {
    Value { name: String, obj: &'a KObject<'a> },
    Function { name: String, fn_ref: &'a KFunction<'a>, obj: &'a KObject<'a> },
    /// Queued `Scope::register_type` retry. Mirrors `Bindings::try_register_type`'s
    /// argument shape; the variant tag preserves the `types`-map collision check on
    /// retry (a single shared retry path would collapse with `Value` and lose the
    /// `data`-vs-`types` storage distinction).
    Type { name: String, kt: &'a crate::runtime::machine::model::types::KType },
}

/// Queue of writes deferred when their `try_borrow_mut` collided. Owned by [`super::scope::Scope`]
/// by value; `defer_value` / `defer_function` mirror the [`Bindings`] write surface, and
/// `drain` takes `&Bindings<'a>` so retries route through the same validated write path as
/// direct writes (the dual-map invariant — every `KFunction` in `data` is mirrored into
/// the `functions` bucket — applies to drained retries by construction).
pub struct PendingQueue<'a> {
    pending: RefCell<Vec<PendingWrite<'a>>>,
}

impl<'a> PendingQueue<'a> {
    pub fn new() -> Self {
        Self { pending: RefCell::new(Vec::new()) }
    }

    /// Queue a LET-style value bind for retry. Mirrors [`Bindings::try_bind_value`]'s
    /// argument shape so the caller's try-then-defer site is symmetric.
    pub fn defer_value(&self, name: String, obj: &'a KObject<'a>) {
        self.pending.borrow_mut().push(PendingWrite::Value { name, obj });
    }

    /// Queue an FN-style overload registration for retry. Mirrors
    /// [`Bindings::try_register_function`]'s argument shape.
    pub fn defer_function(&self, name: String, fn_ref: &'a KFunction<'a>, obj: &'a KObject<'a>) {
        self.pending.borrow_mut().push(PendingWrite::Function { name, fn_ref, obj });
    }

    /// Queue a `Scope::register_type` retry. Mirrors [`Bindings::try_register_type`]'s
    /// argument shape so the caller's try-then-defer site is symmetric.
    pub fn defer_type(&self, name: String, kt: &'a crate::runtime::machine::model::types::KType) {
        self.pending.borrow_mut().push(PendingWrite::Type { name, kt });
    }

    /// Apply queued writes through `bindings` between dispatch nodes. Items that still hit
    /// a borrow conflict re-queue (eventually-consistent, not guaranteed-empty after one
    /// call).
    ///
    /// **Drain-time `Err` policy.** By drain time these are invariant violations — direct
    /// writes already rejected semantically-bad bindings at submission, so anything that
    /// surfaces an `Err` on retry indicates a queue/dispatch interaction bug (e.g. a
    /// drained `Value` write whose `data[name]` was claimed by a different non-function
    /// between queueing and drain). Debug builds `debug_assert!` to surface the bug
    /// immediately; release builds keep the historical `Err(_)`-drop behavior so dispatch
    /// nodes never see surfaced errors.
    ///
    /// `std::mem::take` is load-bearing: [`Bindings::try_apply`] may itself contend and
    /// trigger a re-entrant `defer_*` during retry, so the queue must move out before the
    /// loop or the inner borrow would deadlock.
    pub fn drain(&self, bindings: &Bindings<'a>) {
        if self.pending.borrow().is_empty() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        let mut still_pending: Vec<PendingWrite<'a>> = Vec::new();
        for item in pending {
            match item {
                PendingWrite::Value { name, obj } => {
                    match bindings.try_bind_value(&name, obj) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Value { name, obj });
                        }
                        // `_e` (not `e`) so the release build's `debug_assert!` no-op
                        // doesn't trip the unused-variable lint — the format string only
                        // evaluates `e` in debug.
                        Err(_e) => {
                            debug_assert!(
                                false,
                                "PendingQueue::drain hit invariant violation: {_e}",
                            );
                        }
                    }
                }
                PendingWrite::Function { name, fn_ref, obj } => {
                    match bindings.try_register_function(&name, fn_ref, obj) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Function { name, fn_ref, obj });
                        }
                        Err(_e) => {
                            debug_assert!(
                                false,
                                "PendingQueue::drain hit invariant violation: {_e}",
                            );
                        }
                    }
                }
                PendingWrite::Type { name, kt } => {
                    match bindings.try_register_type(&name, kt) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Type { name, kt });
                        }
                        Err(_e) => {
                            debug_assert!(
                                false,
                                "PendingQueue::drain hit invariant violation: {_e}",
                            );
                        }
                    }
                }
            }
        }
        if !still_pending.is_empty() {
            self.pending.borrow_mut().extend(still_pending);
        }
    }
}

impl<'a> Default for PendingQueue<'a> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    //! Stage-1.4 coverage for the new `Type` variant on the deferred-write queue.
    //! Mirrors the structure of the scope-side `add_during_active_data_borrow_queues_and_drains`
    //! test (`tests.rs`), but reaches directly into `PendingQueue` + `Bindings` so the
    //! property under test is the queue/drain round-trip rather than any scope-layer wiring.

    use super::*;
    use crate::runtime::machine::core::arena::RuntimeArena;
    use crate::runtime::machine::model::types::KType;

    /// `defer_type` queues a write; `drain` replays it through `try_register_type` and
    /// lands the `&KType` in the `types` map. Pins the symmetry with `defer_value` /
    /// `defer_function` — the new variant participates in the same drain loop without a
    /// dedicated entry point.
    #[test]
    fn defer_type_queues_and_drain_replays_into_types() {
        let arena = RuntimeArena::new();
        let bindings: Bindings<'_> = Bindings::new();
        let queue: PendingQueue<'_> = PendingQueue::new();
        let kt = arena.alloc_ktype(KType::Number);
        queue.defer_type("Foo".to_string(), kt);
        // Pre-drain: types map empty.
        assert!(bindings.types().get("Foo").is_none());
        queue.drain(&bindings);
        // Post-drain: replayed through try_register_type into the types map.
        let stored = *bindings.types().get("Foo").expect("Foo should be in types after drain");
        assert!(std::ptr::eq(stored, kt));
    }
}
