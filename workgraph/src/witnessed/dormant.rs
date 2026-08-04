//! The dormant slot every resting carrier stores its lifetime-erased value in, and the owned
//! resting tier built over it.
//!
//! A dormant carrier's value is *not* live: nothing may be assumed about it until a witnessed
//! re-anchor. A struct field typed as a reference says the opposite to the abstract machine — a
//! function-entry retag descends into by-value aggregate arguments and *protects* every reference
//! it finds, so a by-value carrier whose own pins hold the last `Rc` on the region its contents
//! point into would deallocate memory carrying a protected tag when those pins drop in-call.
//! Retag does not descend into unions, which is what keeps a by-value carrier drop sound: the slot
//! is a one-field union, [`Dormant<V>`], so a resting value carries no protected tag.
//!
//! A union field has no drop glue and `Copy + Drop` cannot share a type, so the resting surface
//! splits in two tiers. The **Copy tier** — [`Erased`](super::Erased), [`Sealed`](super::Sealed),
//! [`SealedExtern`](super::SealedExtern) and everything built over them — bounds its family
//! `T:` [`DropFree`](super::DropFree) and stores a bare [`Dormant`]. A **droppable** family rests on
//! the owned tier, [`SealedPinned`], whose slot is [`DormantGlue`] (the value's drop glue runs)
//! and which co-locates the pins covering the value at its erase door.
//!
//! This module is the single audited home for union reads. Every one of them leans on one
//! invariant:
//!
//! > The slot is always initialized: the only constructor initializes `value`, no method
//! > deinitializes it without consuming the wrapper, and the union has no other field.

use std::mem::ManuallyDrop;

use super::{erase_to_static, retype, DropFree, Reattachable, SealedExtern, Witness};

/// The glue-free dormant slot: a one-field union holding a lifetime-erased value.
///
/// The union is what removes the resting value's protected tag (see the module docs). It carries
/// **no** `repr` attribute — `transparent_unions` is unstable, and nothing depends on the layout:
/// every retype operates on a value moved out of the slot, never on the slot itself.
pub(in crate::witnessed) union Dormant<V> {
    value: ManuallyDrop<V>,
}

impl<V> Dormant<V> {
    /// Put a value to rest in the slot.
    pub(in crate::witnessed) fn new(value: V) -> Self {
        Dormant {
            value: ManuallyDrop::new(value),
        }
    }

    /// Borrow the dormant value.
    pub(in crate::witnessed) fn get(&self) -> &V {
        // SAFETY: the always-initialized invariant (module docs) — the slot's only field is live.
        unsafe { &self.value }
    }

    /// Move the dormant value out. `Dormant` has no drop glue, so the moved-out value is the sole
    /// owner of whatever it holds.
    pub(in crate::witnessed) fn into_inner(self) -> V {
        // SAFETY: the always-initialized invariant; consuming `self` means no second read can
        // observe the moved-from slot.
        unsafe { ManuallyDrop::into_inner(self.value) }
    }
}

// Manual impls: a union cannot derive either, and the bound is on the stored value, not the slot.
impl<V: Clone> Clone for Dormant<V> {
    fn clone(&self) -> Self {
        Dormant::new(self.get().clone())
    }
}
impl<V: Copy> Copy for Dormant<V> {}

/// A dormant slot whose value's drop glue still runs — the owned tier's slot. It is the same union
/// (so the same protected-tag-free rest), wrapped in the one type that owns the value's
/// destructor.
pub(in crate::witnessed) struct DormantGlue<V>(Dormant<V>);

impl<V> DormantGlue<V> {
    /// Put a value to rest in the slot, keeping its drop glue.
    pub(in crate::witnessed) fn new(value: V) -> Self {
        DormantGlue(Dormant::new(value))
    }

    /// Move the dormant value out, handing ownership — and the destructor obligation — to the
    /// caller.
    pub(in crate::witnessed) fn into_inner(self) -> V {
        let mut this = ManuallyDrop::new(self);
        // SAFETY: the always-initialized invariant; `this` is a `ManuallyDrop`, so the `Drop` impl
        // below cannot run after this take — the value is moved out exactly once.
        unsafe { ManuallyDrop::take(&mut this.0.value) }
    }
}

impl<V> Drop for DormantGlue<V> {
    fn drop(&mut self) {
        // SAFETY: the always-initialized invariant; drop runs at most once, and never after
        // `into_inner`, which forgets `self` before taking the value.
        unsafe { ManuallyDrop::drop(&mut self.0.value) }
    }
}

/// The **internally witnessed** dormant form for a droppable family: the slot keeps its drop glue
/// and the pins that cover the value are bundled at the erase door. Where
/// [`SealedExtern`] is externally witnessed — the pin supplied at the open — a `SealedPinned` owns
/// its pin for its whole dormant life, so dropping it unopened is sound: the value's glue runs
/// while the pins still hold every region it reads.
///
/// This is the resting tier for a family whose `At<'static>` needs drop — a boxed continuation, an
/// accumulator owning heap contents — which the Copy tier's `DropFree` bound excludes. A droppable
/// *and* region-pointing family is the shape it exists for.
pub struct SealedPinned<T: Reattachable, W: Witness> {
    // Field order is load-bearing: struct fields drop in declaration order, so the value's drop
    // glue runs while `pins` is still alive — a droppable family's drop may freely dereference
    // region memory.
    value: DormantGlue<T::At<'static>>,
    pins: W,
}

impl<T: Reattachable, W: Witness> SealedPinned<T, W> {
    /// Seal a live droppable carrier together with the pins that cover it, in one act — so a
    /// droppable erased value never exists without its glue and its pins, unwind included.
    ///
    /// The caller's obligation is the [`Witness`] contract, discharged once here instead of at
    /// every open: `pins` keeps every region `live` reaches alive and fixed-address for as long as
    /// the returned seal is held. Safe for the same reason [`Erased::erase`](super::Erased::erase)
    /// is — forgetting a lifetime for storage cannot fabricate one.
    pub fn erase(live: T::At<'_>, pins: W) -> Self {
        SealedPinned {
            value: DormantGlue::new(erase_to_static::<T>(live)),
            pins,
        }
    }

    /// Consume the seal at a **rank-2** (`for<'b>`) brand, re-anchoring a zipped [`SealedExtern`]
    /// operand at the **same** brand — the step-open shape, where an embedder's continuation opens
    /// beside its scope and dep operands and an invariant family rejects separately-branded opens.
    /// This is the tier's **only** open verb: a caller with no extern operand passes a trivial one.
    ///
    /// The bundled pins cover the owned value; `operand_pin` covers the extern operand (the
    /// [`SealedExtern::open`] obligation, unchanged). Both live values are consumed by `f` before
    /// the pins drop, and the `for<'b>` quantifier keeps either from escaping into `R`.
    pub fn open<U: Reattachable + DropFree, Wx: Witness, R>(
        self,
        operand: SealedExtern<U>,
        operand_pin: &Wx,
        f: impl for<'b> FnOnce(T::At<'b>, U::At<'b>) -> R,
    ) -> R {
        // Destructuring by move is legal — `SealedPinned` has no `Drop` of its own — and binding
        // `pins` before any live value makes the locals drop live-value-then-pins on every path.
        let SealedPinned { value, pins } = self;
        let erased: T::At<'static> = value.into_inner();
        // SAFETY: `pins` (bound above, dropped below after `f` returns) pins the owned value's
        // backing for the whole call — the `Witness` contract the erase door bundled; the borrowed
        // `operand_pin` pins the operand's, exactly as `SealedExtern::open` requires. Both carriers
        // re-anchor to one fresh existential `'b` the `for<'b>` closure cannot leak (the shape
        // `Witnessed::merge_composed` shares), and both live values move into `f`, so on unwind
        // they drop inside `f`'s frame — before `pins`, which lives in this one. Lifetime-only
        // retype of single-lifetime families (the `Reattachable` contract).
        let live: T::At<'_> = unsafe { retype::<T::At<'static>, T::At<'_>>(erased) };
        let live_operand: U::At<'_> = unsafe { operand.into_erased().reattach() };
        let result = f(live, live_operand);
        let _ = operand_pin;
        drop(pins);
        result
    }
}
