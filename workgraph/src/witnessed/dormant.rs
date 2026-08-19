//! The dormant slot every resting carrier stores its lifetime-erased value in, and the owned
//! resting tier built over it ([design/witnessed-memory.md § The dormant slot and the two resting
//! tiers](../../design/witnessed-memory.md#the-dormant-slot-and-the-two-resting-tiers)).
//!
//! The slot is a one-field union, [`Dormant<V>`], because a function-entry retag does not descend
//! into unions: a resting value carries no protected tag, so a by-value carrier whose own pins
//! hold the last `Rc` on the region its contents point into is sound to drop in-call. A union
//! field has no drop glue, so a droppable family rests on the owned tier, [`SealedPinned`], over
//! [`DormantGlue`] — the same union wrapped in the one type that owns the value's destructor.
//!
//! This module is the single audited home for union reads, each leaning on one invariant:
//!
//! > The slot is always initialized: the only constructor initializes `value`, no method
//! > deinitializes it without consuming the wrapper, and the union has no other field.

use std::marker::PhantomData;
use std::mem::ManuallyDrop;

use super::{DropFree, Reattachable, SealedExtern, Witness, erase_to_static, retype};

/// The glue-free dormant slot: a one-field union holding a lifetime-erased value.
///
/// The union is what removes the resting value's protected tag (see the module docs). It carries
/// **no** `repr` attribute — `transparent_unions` is unstable, and nothing depends on the layout:
/// every retype operates on a value moved out of the slot, never on the slot itself.
pub(in crate::witnessed) union Dormant<V> {
    value: ManuallyDrop<V>,
}

impl<V> Dormant<V> {
    pub(in crate::witnessed) fn new(value: V) -> Self {
        Dormant {
            value: ManuallyDrop::new(value),
        }
    }

    pub(in crate::witnessed) fn get(&self) -> &V {
        // SAFETY: the always-initialized invariant (module docs) — the slot's only field is live.
        unsafe { &self.value }
    }

    /// `Dormant` has no drop glue, so the moved-out value is the sole owner of whatever it holds.
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
    pub(in crate::witnessed) fn new(value: V) -> Self {
        DormantGlue(Dormant::new(value))
    }

    /// Hands the destructor obligation to the caller along with the value.
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

/// Proof that an open's rank-2 brand `'b` sits inside the caller-visible `'outer` — the declared
/// `'outer: 'b` bound is the fact the token carries. [`SealedPinned::open`] hands one to its
/// closure, and the HRTB instantiation must discharge the bound, so `'b` can no longer be
/// instantiated at `'static` behind the caller's back: inside the closure, a **covariant**
/// `'outer`-lifetime value the closure captures shortens to the brand by ordinary subtyping.
///
/// This is the channel for an ambient capability that is a live, borrow-checked reference for the
/// whole of `'outer` (an embedder's run-long allocation brand, say): such a value needs no seal, no
/// re-anchor and no pin — its liveness is the borrow checker's, not the witness system's — it only
/// needs the outlives fact the quantifier would otherwise erase. A value whose lifetime was erased
/// enters through the sealed operand instead. The shape is `std::thread::scope`'s: the closure's
/// `'scope` is bounded by `'env` through the declared bound on its argument type.
///
/// An invariant family is unaffected: nothing unifies `'b` with `'outer` — the closure must still
/// typecheck for every `'b` inside it — and the `for<'b>` quantifier still keeps every
/// `'b`-branded value from escaping into the result.
#[derive(Clone, Copy)]
pub struct Within<'b, 'outer: 'b> {
    _bound: PhantomData<&'b &'outer ()>,
}

/// The **internally witnessed** dormant form for a droppable family: the slot keeps its drop glue
/// and the pins that cover the value are bundled at the erase door, so dropping the seal unopened
/// is sound — the value's glue runs while the pins still hold every region it reads. Where
/// [`SealedExtern`] is externally witnessed, its pin supplied at each open, a `SealedPinned` owns
/// its pin for its whole dormant life ([design/witnessed-memory.md § What a droppable family
/// accepts](../../design/witnessed-memory.md#what-a-droppable-family-accepts)).
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
    /// [`SealedExtern::open`] obligation). Both live values are consumed by `f` before the pins
    /// drop, and the `for<'b>` quantifier keeps either from escaping into `R`.
    ///
    /// The brand is bounded by the caller's `'outer` through the [`Within`] token the closure
    /// receives, so an ambient covariant value live for all of `'outer` shortens to `'b` inside the
    /// closure with no carrier — see [`Within`] for why that channel is borrow-checked rather than
    /// witnessed.
    pub fn open<'outer, U: Reattachable + DropFree, Wx: Witness, R>(
        self,
        operand: SealedExtern<U>,
        operand_pin: &Wx,
        f: impl for<'b> FnOnce(Within<'b, 'outer>, T::At<'b>, U::At<'b>) -> R,
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
        let result = f(
            Within {
                _bound: PhantomData,
            },
            live,
            live_operand,
        );
        let _ = operand_pin;
        drop(pins);
        result
    }
}
