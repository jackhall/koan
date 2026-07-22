//! [`RecordSubstrate`] — the region-resident field substrate behind a record value: the field
//! [`Record`] plus its construction-time contains-borrows memo. The wrapper is the pattern every
//! later container conversion in this project copies; see
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
}

impl<'a> RecordSubstrate<'a> {
    /// Build from a fields record and its precomputed contains-borrows bit. The join pass that
    /// derives `contains_borrows` from `fields` lives at the construction site, not here — this is
    /// the door's own plain constructor.
    pub(crate) fn new(fields: Record<Held<'a>>, contains_borrows: bool) -> Self {
        RecordSubstrate {
            fields,
            contains_borrows,
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
}
