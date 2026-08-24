//! Surface-name and `TypeIdentifier` → `KType` elaboration.
//!
//! Join (least upper bound) and union canonicalization live on
//! [`TypeRegistry`](super::registry::TypeRegistry), which is where interning happens.

use super::builtin_names::builtin_types;
use super::ktype::KType;
use crate::machine::model::labels::TypeSymbol;

impl KType {
    /// Look up a `KType` by the name a user can write in source (e.g. `Number`, `List`). Every
    /// name here lowers to a fixed handle, so the lookup needs no registry: the content each one
    /// names is pre-seeded into every registry at construction.
    ///
    /// Eleven symbol compares against the memoized [`builtin_types`] table — no hashing, no
    /// allocation.
    pub fn from_symbol(name: TypeSymbol) -> Option<KType> {
        builtin_types()
            .into_iter()
            .find(|(declared, _)| declared.symbol() == name)
            .map(|(_, ktype)| ktype)
    }

    /// [`from_symbol`](Self::from_symbol) for a name that arrives as source text. Classifies and
    /// hashes `name` without interning it; a name that is not a Type token misses the table.
    pub fn from_name(name: &str) -> Option<KType> {
        KType::from_symbol(TypeSymbol::of(name)?)
    }
}

#[cfg(test)]
mod tests;
