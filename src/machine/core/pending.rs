//! Deferred-write queue: writes whose `try_borrow_mut` collides are queued here and
//! replayed by [`PendingQueue::drain`] through the same validated [`Bindings`] write
//! path as direct writes, so the function-mirror invariant extends to drained writes
//! by construction.

use std::cell::RefCell;

use std::rc::Rc;

use crate::machine::core::arena::FrameStorage;
use crate::machine::core::carrier_witness::SealedFunction;

use super::bindings::{ApplyOutcome, BindingIndex, Bindings, DeclarationSite, SealedValue};

/// The variant tag is load-bearing: it routes each retry through the matching
/// `Bindings::try_*` so per-map collision checks (function-mirror, `types` vs `data`)
/// stay intact. A value/function write carries the original [`BindingIndex`] and a type
/// write its [`DeclarationSite`], so the drained write lands under the same position and
/// declaration identity the conflicted write would have used.
enum PendingWrite {
    Value {
        name: String,
        index: BindingIndex,
        /// The bound value fused to its exact reach, carried through the deferred write so a
        /// drained bind stores exactly what a direct bind would (see [`Bindings::try_bind_value`]).
        sealed: SealedValue,
        /// The value's mirror seal, if it wraps a callable — the mirror the drained write replays.
        function: Option<SealedFunction>,
    },
    Function {
        name: String,
        fn_ref: SealedFunction,
        index: BindingIndex,
    },
    Type {
        name: String,
        kt: crate::machine::model::KType,
        site: DeclarationSite,
    },
}

pub struct PendingQueue {
    pending: RefCell<Vec<PendingWrite>>,
}

impl PendingQueue {
    pub fn new() -> Self {
        Self {
            pending: RefCell::new(Vec::new()),
        }
    }

    pub fn defer_value(
        &self,
        name: String,
        index: BindingIndex,
        sealed: SealedValue,
        function: Option<SealedFunction>,
    ) {
        self.pending.borrow_mut().push(PendingWrite::Value {
            name,
            index,
            sealed,
            function,
        });
    }

    pub fn defer_function(&self, name: String, fn_ref: SealedFunction, index: BindingIndex) {
        self.pending.borrow_mut().push(PendingWrite::Function {
            name,
            fn_ref,
            index,
        });
    }

    pub fn defer_type(
        &self,
        name: String,
        kt: crate::machine::model::KType,
        site: DeclarationSite,
    ) {
        self.pending
            .borrow_mut()
            .push(PendingWrite::Type { name, kt, site });
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
    pub fn drain(&self, bindings: &Bindings<'_>, pin: &Rc<FrameStorage>) {
        if self.pending.borrow().is_empty() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        let mut still_pending: Vec<PendingWrite> = Vec::new();
        for item in pending {
            match item {
                PendingWrite::Value {
                    name,
                    index,
                    sealed,
                    function,
                } => {
                    // Duplicate the seal for the attempt, keeping the original for a re-defer on a
                    // repeat conflict, mirroring the direct-bind path (the duplicate preserves the
                    // value ⇔ reach pairing).
                    match bindings.try_bind_value(
                        &name,
                        index,
                        sealed.duplicate(),
                        function.as_ref().map(SealedFunction::duplicate),
                        pin,
                    ) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Value {
                                name,
                                index,
                                sealed,
                                function,
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
                    index,
                } => match bindings.try_register_function(&name, fn_ref.duplicate(), index, pin) {
                    Ok(ApplyOutcome::Applied) => {}
                    Ok(ApplyOutcome::Conflict) => {
                        still_pending.push(PendingWrite::Function {
                            name,
                            fn_ref,
                            index,
                        });
                    }
                    Err(_e) => {
                        debug_assert!(false, "PendingQueue::drain hit invariant violation: {_e}",);
                    }
                },
                PendingWrite::Type { name, kt, site } => {
                    match bindings.try_register_type(&name, kt, site) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Type { name, kt, site });
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

impl Default for PendingQueue {
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
        let storage = crate::machine::core::run_root_storage();
        let bindings: Bindings<'_> = Bindings::new();
        let queue: PendingQueue = PendingQueue::new();
        let kt = KType::NUMBER;
        queue.defer_type("Foo".to_string(), kt, DeclarationSite::BUILTIN);
        assert!(bindings.types().get("Foo").is_none());
        queue.drain(&bindings, &storage);
        let stored = bindings
            .types()
            .get("Foo")
            .expect("Foo should be in types after drain")
            .0;
        assert_eq!(stored, kt);
    }

    #[test]
    fn default_yields_empty_queue() {
        let storage = crate::machine::core::run_root_storage();
        let queue: PendingQueue = PendingQueue::default();
        let bindings: Bindings<'_> = Bindings::new();
        queue.drain(&bindings, &storage);
        assert!(bindings.data().is_empty());
        assert!(bindings.types().is_empty());
    }
}
