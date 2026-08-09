//! The re-home token for sectioned cells.
//!
//! A container door's per-cell reach verdict reads a `KString` as `CellReach::Owned` — exact only
//! because the door re-bumped the string's bytes into its own region first. That is an *ordering*
//! law over two statements in one function, the kind a comment can only ask for. [`Rehomed`] makes
//! it a signature: the verdict reader takes a token no one can produce except by running the
//! re-home mint, so skipping or reordering the re-home is a type error rather than a dangling
//! `&'a str` the next region teardown turns into a use-after-free.
//!
//! The token's field is private to this module and the mint is the only constructor, so the audited
//! surface is one function body.

use crate::machine::core::SubstrateDoor;

use super::carried::Held;
use super::kobject::KObject;

/// A cell whose own top-node bytes are resident in the door that minted this token. Carries no
/// evidence of *which* door — region identity is runtime data, not a lifetime (a fold engine
/// legitimately hands producer pointers at the destination brand for reach-covered leaves) — so
/// what it proves is that the re-home ran, not where it landed. That is the half a signature can
/// carry, and the half the ordering bug lives in.
pub(crate) struct Rehomed<'a>(Held<'a>);

impl<'a> Rehomed<'a> {
    /// Re-bump `cell`'s **own** string bytes into `door`'s region and mint the token. The sole
    /// constructor.
    ///
    /// Top-node only, and that is the whole rule: a **nested substrate** cell's own strings are
    /// already home-resident in that substrate's region, which its stored reach union names, so the
    /// pinned-cell verdict covers them and re-walking would be a deep copy this door does not do. A
    /// `Tagged`'s tag rides its own substrate's region for the same reason (`KObject::tagged`
    /// re-bumped it there).
    ///
    /// The cost is one memcpy per string per container construction — what the `String::clone` in a
    /// cell's `deep_clone` cost at these same sites before strings moved into the region.
    pub(crate) fn mint(door: SubstrateDoor<'a, '_>, cell: Held<'a>) -> Self {
        Rehomed(match cell {
            Held::Object(KObject::KString(s)) => {
                Held::Object(KObject::KString(door.allocator().text(s)))
            }
            other => other,
        })
    }

    /// The re-homed cell, for the reach verdict to read its stored facts off.
    pub(crate) fn cell(&self) -> &Held<'a> {
        &self.0
    }

    /// The re-homed cell, to store.
    pub(crate) fn into_cell(self) -> Held<'a> {
        self.0
    }
}
