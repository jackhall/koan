//! Guard-fixture surface for the fold-provenance `compile_fail` tests in
//! `tests/compile_fail/`. Those fixtures compile as **external crates** — they see only
//! koan's public API — yet the fold machinery (`StepContext::alloc_carried_with`, the
//! folded-placement sinks) is `pub(crate)`. This module exposes the **minimum** to drive a
//! fold from outside so a guard can attempt the smuggle the tied sinks forbid.
//!
//! Deliberately narrow: it hands out a fold brand only *through* a real `alloc_carried_with`
//! (the [`drive_object_fold`] wrapper), so the [`FoldingBrand`] the guard's closure receives is
//! one a combinator minted — never a way to forge a brand or a `FoldToken` directly.
//! `FoldingBrand::in_fold_closure` (its sole constructor) stays `pub(crate)`, so an external
//! guard can name the brand but cannot construct one. [`store_folded_object`] re-exposes the
//! `pub(crate)` sink with its brand-lifetime tie intact, since that tie is exactly what the
//! guard pins. `KType` is owned data with no ambient lifetime to smuggle, so this fixture only
//! covers the object channel.
//!
//! `#[doc(hidden)]` and `pub` only because trybuild fixtures import it; it is not part of
//! koan's real surface.

use crate::machine::core::FrameStorageExt;
use crate::machine::execute::drive_step_allocator;
use crate::machine::run_root_storage;

pub use crate::machine::core::{FoldingBrand, FrameStorage};
pub use crate::machine::model::{Carried, KObject};
pub use crate::machine::DeliveredCarried;

/// Run `f` with a [`KObject`] borrowed at an **ambient** (non-`'static`) lifetime — an object
/// resident in a fixture-owned region, the exact shape a fold closure must not smuggle into the
/// object sink. A guard tries to feed this borrow to the tied sink inside a `for<'b>` fold closure.
pub fn with_ambient_object<R>(f: impl FnOnce(&KObject<'_>) -> R) -> R {
    let storage = run_root_storage();
    let object = storage.brand().alloc_object(KObject::Number(1.0));
    f(object)
}

/// Drive a real `alloc_carried_with` object fold over `deps`: `build` runs inside the combinator's
/// `for<'b>` closure with the fold brand and the deps' views, and its `&KObject` result is sealed
/// as the step's object carrier. A guard's `build` attempts to store an ambient-captured object; a
/// compiling twin folds `views` at the brand.
pub fn drive_object_fold<F>(deps: &[&DeliveredCarried], build: F)
where
    F: for<'b> FnOnce(FoldingBrand<'b>, &[Carried<'b>]) -> &'b KObject<'b>,
{
    drive_step_allocator(|ctx| {
        let _ = ctx.alloc_carried_with(deps, |brand, views| Carried::Object(build(brand, &views)));
    });
}

/// Forward to the tied object sink `FoldingBrand::alloc_object_folded`, preserving its
/// brand-lifetime tie (`o: KObject<'b>` on `FoldingBrand<'b>`) — the `pub(crate)` sink is
/// otherwise unnameable from an external guard. Feeding a `KObject` at any lifetime other than
/// the brand's `'b` is the compile error the guard pins.
pub fn store_folded_object<'b>(brand: FoldingBrand<'b>, o: KObject<'b>) -> &'b KObject<'b> {
    brand.alloc_object_folded(o)
}
