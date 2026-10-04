//! How a run of a signature's chained operators reduces.
//!
//! A signature declares a chaining record over a set of operators, and that declaration is part of
//! the signature's identity — [`schema_content_digest`](super::digest::schema_content_digest)
//! feeds a record's mode, so two interfaces differing only in how a run chains are two interfaces.
//! The vocabulary is therefore the lattice's; the operator registry imports it from here.

use crate::symbols::KeywordSymbol;

/// Which way a fold nests a run of more than two operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldDirection {
    /// `a ⊙ b ⊙ c` ⇒ `(a ⊙ b) ⊙ c`.
    Left,
    /// `a ⊙ b ⊙ c` ⇒ `a ⊙ (b ⊙ c)`.
    Right,
}

/// How a recognized run of a group's operators reduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionMode {
    /// The whole operand run is handed to one body as a single list operand.
    Unary,
    /// A binary body folds the run left-associated: `a - b - c` ⇒ `(a - b) - c`.
    FoldLeft,
    /// Right-associated: `a ^ b ^ c` ⇒ `a ^ (b ^ c)`.
    FoldRight,
    /// Each adjacent pair dispatches through its own operator's binary body; the pair
    /// results fold through the group's combiner in the declared direction.
    Pairwise {
        /// The **keyword** of the operator the pair results fold through. It is a keyword symbol,
        /// not a resolved function: the ordinary scope walk resolves it at the chain's use site.
        combiner: KeywordSymbol,
        direction: FoldDirection,
    },
}

impl FoldDirection {
    /// The byte the digest and the canonical order both read.
    pub(super) fn byte(self) -> u8 {
        match self {
            FoldDirection::Left => 0,
            FoldDirection::Right => 1,
        }
    }
}

impl ReductionMode {
    /// The mode's total order and its digest encoding in one: a discriminant byte, a pairwise
    /// mode's combiner, and its direction byte. The digest feeds exactly these, in this order, so
    /// the two cannot drift.
    pub(super) fn canonical_key(self) -> (u8, Option<KeywordSymbol>, u8) {
        match self {
            ReductionMode::Unary => (0, None, 0),
            ReductionMode::FoldLeft => (1, None, 0),
            ReductionMode::FoldRight => (2, None, 0),
            ReductionMode::Pairwise {
                combiner,
                direction,
            } => (3, Some(combiner), direction.byte()),
        }
    }
}
