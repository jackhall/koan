//! Guard-fixture surface for the step-brand `compile_fail` tests in `tests/compile_fail/`. Those
//! fixtures compile as **external crates** — they see only koan's public API — yet
//! [`StepCarried`](crate::machine::execute::StepCarried) and its sole exit
//! [`StepCarried::seal_at_step`](crate::machine::execute::StepCarried) are `pub(crate)` /
//! `pub(super)`. This module exposes the **minimum** to hand a guard a step-branded carrier so it
//! can attempt the smuggle the brand forbids.
//!
//! Deliberately narrow: [`drive_step`] hands the carrier to a `for<'b>` closure — the same rank-2
//! shape the run loop's step tail runs the continuation in — so the `'b` the guard sees is
//! universally quantified and unnameable. A guard that tries to store the carrier past the closure
//! makes `'b` escape, which the borrow checker rejects. There is no accessor returning the
//! lifetime-free carrier, and `seal_at_step` (the sole exit) stays out of an external crate's reach.
//!
//! `#[doc(hidden)]` and `pub` only because trybuild fixtures import it; it is not part of koan's
//! real surface.

use crate::machine::core::FrameStorageExt;
use crate::machine::run_root_storage;

pub use crate::machine::core::StepAllocator;
pub use crate::machine::execute::{drive_step_allocator, StepCarried};
pub use crate::machine::model::KObject;

/// Hand a step-branded carrier to `guard` at a `for<'b>` rank-2 brand — the step tail's confinement
/// shape. The carrier is a real empty-witness object carrier wrapped as a
/// [`StepCarried`](crate::machine::execute::StepCarried); `'b` is universally quantified over
/// `guard`, so a guard body can use the carrier within the closure but cannot store it past it
/// (doing so makes `'b` escape — the `compile_fail` pin). The legal shape — using the carrier inside
/// the closure — compiles.
///
/// The carrier's `born` (`pub(crate)`) and `seal_at_step` (`pub(super)`) are unreachable from an
/// external crate, so a guard can neither forge nor unwrap a carrier: the smuggle fails on the brand
/// lifetime, and the unwrap is unreachable.
pub fn drive_step(guard: impl for<'b> FnOnce(StepCarried<'b>)) {
    let storage = run_root_storage();
    guard(storage.brand().alloc_object_witnessed(KObject::Number(1.0)));
}
