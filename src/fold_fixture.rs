//! Guard-fixture surface for the fold-provenance `compile_fail` tests in
//! `tests/compile_fail/`. Those fixtures compile as **external crates** — they see only
//! koan's public API — yet the fold machinery (`StepContext::alloc_carried_with`, the
//! folded-placement sinks) is `pub(crate)`. This module exposes the **minimum** to drive a
//! fold from outside so a guard can attempt the smuggle the tied sinks forbid.
//!
//! Deliberately narrow: it hands out a fold brand only *through* a real `alloc_carried_with`
//! (the [`drive_type_fold`] / [`drive_object_fold`] wrappers), so the [`FoldingBrand`] the
//! guard's closure receives is one a combinator minted — never a way to forge a brand or a
//! `FoldToken` directly. `FoldingBrand::in_fold_closure` (its sole constructor) stays
//! `pub(crate)`, so an external guard can name the brand but cannot construct one. The
//! `store_folded_*` forwarders re-expose the `pub(crate)` sinks with their brand-lifetime tie
//! intact, since that tie is exactly what the guards pin.
//!
//! `#[doc(hidden)]` and `pub` only because trybuild fixtures import it; it is not part of
//! koan's real surface.

use crate::machine::core::FrameStorageExt;
use crate::machine::execute::drive_step_allocator;
use crate::machine::{run_root_storage, CarrierWitness};
use crate::witnessed::{Delivered, Erased, Witnessed};

pub use crate::machine::core::{FoldingBrand, FrameStorage};
pub use crate::machine::model::types::KType;
pub use crate::machine::model::{Carried, KObject};
pub use crate::machine::DeliveredCarried;

/// A delivered type terminal (a `Number` type resident in its own frame's region), the
/// operand a compiling twin folds from at the brand.
pub fn deliver_type() -> DeliveredCarried {
    let storage = run_root_storage();
    let kt = storage.brand().alloc_ktype(KType::Number);
    Delivered::seal(
        Witnessed::from_erased(Erased::erase(Carried::Type(kt)), CarrierWitness::default()),
        storage,
    )
}

/// Run `f` with a [`KType`] borrowed at an **ambient** (non-`'static`) lifetime — a type
/// resident in a fixture-owned region, the exact shape a fold closure must not smuggle in.
/// A guard tries to feed this borrow to the tied type sink inside a `for<'b>` fold closure.
pub fn with_ambient_type<R>(f: impl FnOnce(&KType<'_>) -> R) -> R {
    let storage = run_root_storage();
    let kt = storage.brand().alloc_ktype(KType::Number);
    f(kt)
}

/// [`with_ambient_type`]'s object twin: run `f` with a [`KObject`] borrowed at an ambient
/// lifetime, the shape a fold closure must not smuggle into the object sink.
pub fn with_ambient_object<R>(f: impl FnOnce(&KObject<'_>) -> R) -> R {
    let storage = run_root_storage();
    let object = storage.brand().alloc_object(KObject::Number(1.0));
    f(object)
}

/// Drive a real `alloc_carried_with` type fold over `deps`: `build` runs inside the combinator's
/// `for<'b>` closure with the fold brand and the deps' views, and its `&KType` result is sealed
/// as the step's type carrier. A guard's `build` attempts to store an ambient-captured type; a
/// compiling twin folds `views` at the brand.
pub fn drive_type_fold<F>(deps: &[&DeliveredCarried], build: F)
where
    F: for<'b> FnOnce(FoldingBrand<'b>, &[Carried<'b>]) -> &'b KType<'b>,
{
    drive_step_allocator(|ctx| {
        let _ = ctx.alloc_carried_with(deps, |brand, views| Carried::Type(build(brand, &views)));
    });
}

/// [`drive_type_fold`]'s object twin, sealing the `&KObject` result as the step's object carrier.
pub fn drive_object_fold<F>(deps: &[&DeliveredCarried], build: F)
where
    F: for<'b> FnOnce(FoldingBrand<'b>, &[Carried<'b>]) -> &'b KObject<'b>,
{
    drive_step_allocator(|ctx| {
        let _ = ctx.alloc_carried_with(deps, |brand, views| Carried::Object(build(brand, &views)));
    });
}

/// Forward to the tied type sink `FoldingBrand::alloc_ktype_folded`, preserving its
/// brand-lifetime tie (`t: KType<'b>` on `FoldingBrand<'b>`) — the `pub(crate)` sink is
/// otherwise unnameable from an external guard. Feeding a `KType` at any lifetime other than
/// the brand's `'b` is the compile error the guard pins.
pub fn store_folded_type<'b>(brand: FoldingBrand<'b>, t: KType<'b>) -> &'b KType<'b> {
    brand.alloc_ktype_folded(t)
}

/// [`store_folded_type`]'s object twin, forwarding to `FoldingBrand::alloc_object_folded`
/// with the same `o: KObject<'b>` tie.
pub fn store_folded_object<'b>(brand: FoldingBrand<'b>, o: KObject<'b>) -> &'b KObject<'b> {
    brand.alloc_object_folded(o)
}
