//! Label identity: [`Symbol`], the fixed-width handle every syntactic label travels as, and
//! [`LabelInterner`], the run-scoped side table that turns one back into text.
//!
//! A label — a record field name, a struct schema field, an FN parameter name — originates in
//! source text and is fixed at declaration, so its identity is a content digest: the low 128 bits
//! of BLAKE3 over its UTF-8 bytes, the same width and collision footing as a
//! [`TypeDigest`](crate::machine::model::types::TypeDigest). [`Symbol::of`] is a pure function:
//! making a symbol needs no interner, no registry, no execution context, and equal text yields
//! equal symbols in every run.
//!
//! The interner is therefore *not* a lookup authority. Comparisons and probes go straight through
//! symbol bits; the table is written only where a syntactic label is constructed and read only
//! where one is rendered. Its growth is bounded by the run's source text.
//!
//! See [design/label-interning.md](../../../design/label-interning.md).

use std::cell::RefCell;
use std::collections::HashMap;

use super::types::registry::IdentityBuildHasher;

/// A label's content identity: the low 128 bits of a BLAKE3 hash of its UTF-8 bytes.
///
/// `Copy`, lifetime-free, and compared and hashed without touching text. `Ord` is the numeric
/// order of those bits — the canonical field order for digest feeds and record cell layout.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct Symbol(pub u128);

impl Symbol {
    /// The label's digest. Pure — no interner, no allocation, no ambient state.
    pub fn of(text: &str) -> Symbol {
        let hash = blake3::hash(text.as_bytes());
        let low: [u8; 16] = hash.as_bytes()[..16]
            .try_into()
            .expect("BLAKE3 output is 32 bytes");
        Symbol(u128::from_le_bytes(low))
    }
}

/// The run's digest → text side table for labels.
///
/// Interior mutability by `RefCell`, matching the type registry beside it: construction sites hold
/// a shared `&RunRegistries` and still need to record text. Never borrowed across a call that can
/// re-enter — [`intern`](Self::intern) and [`resolve`](Self::resolve) each take and release the
/// borrow within one statement.
#[derive(Default)]
pub struct LabelInterner {
    texts: RefCell<HashMap<Symbol, Box<str>, IdentityBuildHasher>>,
}

impl LabelInterner {
    pub fn new() -> Self {
        LabelInterner::default()
    }

    /// Record `text` under its symbol and hand the symbol back. Insert-if-absent: equal text
    /// already recorded costs one lookup.
    pub fn intern(&self, text: &str) -> Symbol {
        let symbol = Symbol::of(text);
        let mut texts = self.texts.borrow_mut();
        texts.entry(symbol).or_insert_with(|| text.into());
        symbol
    }

    /// The text recorded for `symbol`, or `None` if nothing interned it in this run. Render paths
    /// only — a miss is a rendering placeholder, never an error.
    pub fn resolve(&self, symbol: Symbol) -> Option<String> {
        self.texts
            .borrow()
            .get(&symbol)
            .map(|text| text.to_string())
    }

    /// How many distinct labels this run has recorded.
    pub fn len(&self) -> usize {
        self.texts.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.borrow().is_empty()
    }
}

#[cfg(test)]
mod tests;
