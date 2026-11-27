//! The build module's tests: the folds, the load's store, and the import boundary; and the count
//! of the units each walk types, which the dispatch suite reads.

use std::cell::RefCell;

use crate::scope::Shapes;

mod boundary;
mod folds;
mod load;

thread_local! {
    /// How many units the walks on this thread typed, by the address of the shapes each walked: a
    /// composition walks shapes of its own, so a program's count is its own load's alone.
    static TYPED: RefCell<Vec<(usize, u64)>> = const { RefCell::new(Vec::new()) };
}

/// Starts the count of a walk over the shapes at `shapes` afresh: a later load may lay its shapes
/// where a dropped one's were.
pub(super) fn walking(shapes: usize) {
    TYPED.with(|typed| typed.borrow_mut().retain(|(at, _)| *at != shapes));
}

/// Counts a unit a walk over the shapes at `shapes` typed.
pub(super) fn typed(shapes: usize) {
    TYPED.with(|typed| {
        let mut typed = typed.borrow_mut();
        match typed.iter_mut().find(|(at, _)| *at == shapes) {
            Some((_, count)) => *count += 1,
            None => typed.push((shapes, 1)),
        }
    });
}

/// How many units the walks on this thread have typed of `shapes`.
pub(crate) fn typed_over(shapes: &Shapes<'_>) -> u64 {
    let at = std::ptr::from_ref(shapes) as usize;
    TYPED.with(|typed| {
        (typed.borrow().iter())
            .find(|(shapes, _)| *shapes == at)
            .map_or(0, |(_, count)| *count)
    })
}

/// How many units the walks on this thread have typed, of any shapes.
pub(crate) fn typed_in_all() -> u64 {
    TYPED.with(|typed| typed.borrow().iter().map(|(_, count)| count).sum())
}
