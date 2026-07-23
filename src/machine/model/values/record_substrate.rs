//! [`RecordSubstrate`] — the region-resident field substrate behind a record value: the field
//! [`Record`] plus its three construction-time memos (contains-borrows, copy-cost, borrows-home).
//! The wrapper is the pattern every later container conversion in this project copies; see
//! [design/value-substrates.md](../../../../design/value-substrates.md).

use crate::machine::model::types::Record;

use super::Held;

/// The field substrate a record value borrows. Immutable after construction — no interior field
/// writes exist anywhere in the runtime, so a region-resident substrate needs no mutation story.
/// Born only through the branded door
/// ([`FoldingBrand::alloc_record_folded`](crate::machine::core::FoldingBrand::alloc_record_folded)),
/// which stores the substrate and hands back a co-located borrow — the fields and the memoized bit
/// ride together, so the memo can never go stale relative to its own fields.
pub struct RecordSubstrate<'a> {
    fields: Record<Held<'a>>,
    /// Set iff some transitive cell is a region-borrow leaf (closure, module, non-splice-free
    /// expression) or a still-`Rc` composite (list/dict/tagged/wrapped — carrying no memo of its
    /// own to consult, so the bit is conservative there until each container converts).
    /// Memoized in the same pass that computes the field-type join; never recomputed by a walk.
    contains_borrows: bool,
    /// Exact cost in bytes of totally rebuilding this substrate's reachable structure at a
    /// destination brand. `u64::MAX` (saturated): unpriceable — some transitive cell is a
    /// still-`Rc` composite (list/dict/tagged/wrapped) or a `KExpression`, which carry no memo of
    /// their own until their conversions ship.
    copy_cost: u64,
    /// Whether some transitive borrow leaf points into this substrate's own home region. Exact when
    /// `copy_cost` is priced (leaf regions are O(1) reads at construction; nested records compose
    /// their own bits, co-resident by construction); conservatively `true` alongside an unpriceable
    /// cost.
    borrows_home: bool,
}

impl<'a> RecordSubstrate<'a> {
    /// Build from a fields record and its three precomputed memos. The join pass that derives
    /// `contains_borrows`, `copy_cost`, and `borrows_home` from `fields` lives at the construction
    /// site, not here — this is the door's own plain constructor.
    pub(crate) fn new(
        fields: Record<Held<'a>>,
        contains_borrows: bool,
        copy_cost: u64,
        borrows_home: bool,
    ) -> Self {
        RecordSubstrate {
            fields,
            contains_borrows,
            copy_cost,
            borrows_home,
        }
    }

    /// The field record — declaration-ordered, order-blind equality (see [`Record`]).
    pub fn fields(&self) -> &Record<Held<'a>> {
        &self.fields
    }

    /// Whether some transitive cell is a region-borrow leaf or a still-`Rc` composite — see the
    /// field's own doc.
    pub fn contains_borrows(&self) -> bool {
        self.contains_borrows
    }

    /// Exact cost in bytes of totally rebuilding this substrate at a destination brand, or
    /// `u64::MAX` when unpriceable — see the field's own doc.
    pub fn copy_cost(&self) -> u64 {
        self.copy_cost
    }

    /// Whether some transitive borrow leaf points into this substrate's own home region — see the
    /// field's own doc.
    pub fn borrows_home(&self) -> bool {
        self.borrows_home
    }
}
