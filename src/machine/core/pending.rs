//! Deferred-write queue: writes whose `try_borrow_mut` collides are queued here and
//! replayed by [`PendingQueue::drain`] through the same validated [`Bindings`] write
//! path as direct writes, so the function-mirror invariant extends to drained writes
//! by construction.

use std::cell::RefCell;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::StoredReach;
use crate::machine::model::KObject;

use super::bindings::{ApplyOutcome, BindingIndex, Bindings};

/// The variant tag is load-bearing: it routes each retry through the matching
/// `Bindings::try_*` so per-map collision checks (function-mirror, `types` vs `data`)
/// stay intact. Each variant carries the original [`BindingIndex`] so the drained
/// write lands under the same lexical position the conflicted write would have used.
enum PendingWrite<'a> {
    Value {
        name: String,
        obj: &'a KObject<'a>,
        index: BindingIndex,
        /// The bound value's home-omitted foreign reach, carried through the deferred write so a
        /// drained bind stores the same reach a direct bind would (see [`Bindings::try_bind_value`]).
        reach: StoredReach<'a>,
    },
    Function {
        name: String,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    },
    Type {
        name: String,
        kt: crate::machine::model::KType,
        index: BindingIndex,
    },
}

pub struct PendingQueue<'a> {
    pending: RefCell<Vec<PendingWrite<'a>>>,
}

impl<'a> PendingQueue<'a> {
    pub fn new() -> Self {
        Self {
            pending: RefCell::new(Vec::new()),
        }
    }

    pub fn defer_value(
        &self,
        name: String,
        obj: &'a KObject<'a>,
        index: BindingIndex,
        reach: StoredReach<'a>,
    ) {
        self.pending.borrow_mut().push(PendingWrite::Value {
            name,
            obj,
            index,
            reach,
        });
    }

    pub fn defer_function(
        &self,
        name: String,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) {
        self.pending.borrow_mut().push(PendingWrite::Function {
            name,
            fn_ref,
            obj,
            index,
        });
    }

    pub fn defer_type(&self, name: String, kt: crate::machine::model::KType, index: BindingIndex) {
        self.pending
            .borrow_mut()
            .push(PendingWrite::Type { name, kt, index });
    }

    /// Items that still hit a borrow conflict re-queue (eventually-consistent, not
    /// guaranteed-empty after one call).
    ///
    /// Drain-time `Err` is an invariant violation: direct writes already rejected
    /// semantically-bad bindings at submission, so anything surfacing here is a
    /// queue/dispatch interaction bug. Debug builds `debug_assert!`; release builds
    /// drop the error so dispatch nodes never see it.
    ///
    /// `std::mem::take` is load-bearing: `Bindings::try_*` may itself contend and
    /// re-entrantly `defer_*` during retry, so the queue must move out before the
    /// loop or the inner borrow would deadlock.
    pub fn drain(&self, bindings: &Bindings<'a>) {
        if self.pending.borrow().is_empty() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        let mut still_pending: Vec<PendingWrite<'a>> = Vec::new();
        for item in pending {
            match item {
                PendingWrite::Value {
                    name,
                    obj,
                    index,
                    reach,
                } => {
                    match bindings.try_bind_value(&name, obj, index, reach) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Value {
                                name,
                                obj,
                                index,
                                reach,
                            });
                        }
                        // `_e`: format string only reads it in debug.
                        Err(_e) => {
                            debug_assert!(
                                false,
                                "PendingQueue::drain hit invariant violation: {_e}",
                            );
                        }
                    }
                }
                PendingWrite::Function {
                    name,
                    fn_ref,
                    obj,
                    index,
                } => match bindings.try_register_function(&name, fn_ref, obj, index) {
                    Ok(ApplyOutcome::Applied) => {}
                    Ok(ApplyOutcome::Conflict) => {
                        still_pending.push(PendingWrite::Function {
                            name,
                            fn_ref,
                            obj,
                            index,
                        });
                    }
                    Err(_e) => {
                        debug_assert!(false, "PendingQueue::drain hit invariant violation: {_e}",);
                    }
                },
                PendingWrite::Type { name, kt, index } => {
                    match bindings.try_register_type(&name, kt, index) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Type { name, kt, index });
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
    use super::*;
    use crate::machine::model::KType;

    #[test]
    fn defer_type_queues_and_drain_replays_into_types() {
        let bindings: Bindings<'_> = Bindings::new();
        let queue: PendingQueue<'_> = PendingQueue::new();
        let kt = KType::NUMBER;
        queue.defer_type("Foo".to_string(), kt, BindingIndex::BUILTIN);
        assert!(bindings.types().get("Foo").is_none());
        queue.drain(&bindings);
        let stored = bindings
            .types()
            .get("Foo")
            .expect("Foo should be in types after drain")
            .0;
        assert_eq!(stored, kt);
    }

    #[test]
    fn default_yields_empty_queue() {
        let queue: PendingQueue<'_> = PendingQueue::default();
        let bindings: Bindings<'_> = Bindings::new();
        queue.drain(&bindings);
        assert!(bindings.data().is_empty());
        assert!(bindings.types().is_empty());
    }
}
