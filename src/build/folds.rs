//! The load's **folds**: what the load decides per body once its walk has run, read off the
//! shapes and the walk's records alone — where each body's types intern at run, which names a
//! callable's specialization keys on, and which type captures a body's content composes
//! ([the folds](../../roadmap/gradual-build/one-walk-build.md)). A resumption reads them and never
//! recomputes them.

use crate::memory::{BumpAllocator, Lays};
use crate::scope::{Folds, LoadFacts, ShapeError};

/// The folds of every body `load` holds records for, by segment, laid through `lay`: none yet.
pub fn fold<'o>(
    load: &LoadFacts<'o>,
    lay: impl Lays<'o>,
    scratch: BumpAllocator<'_>,
) -> Result<&'o [Folds<'o>], ShapeError<'o>> {
    let _ = (load, lay, scratch);
    Ok(&[])
}
