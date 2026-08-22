//! Surface-name and `TypeIdentifier` → `KType` elaboration.
//!
//! Join (least upper bound) and union canonicalization live on
//! [`TypeRegistry`](super::registry::TypeRegistry), which is where interning happens.

use super::kkind::KKind;
use super::ktype::KType;

impl KType {
    /// Look up a `KType` by the textual name a user can write in source (e.g. `Number`, `List`).
    /// Every name here lowers to a fixed handle, so the lookup needs no registry: the content
    /// each one names is pre-seeded into every registry at construction.
    pub fn from_name(name: &str) -> Option<KType> {
        match name {
            "Number" => Some(KType::NUMBER),
            "Str" => Some(KType::STR),
            "Bool" => Some(KType::BOOL),
            "Null" => Some(KType::NULL),
            "List" => Some(KType::LIST_OF_ANY),
            "Dict" => Some(KType::DICT_ANY_ANY),
            "KExpression" => Some(KType::KEXPRESSION),
            "Type" => Some(KType::of_kind(KKind::AnyType)),
            "Module" => Some(KType::EMPTY_SIGNATURE),
            "Signature" => Some(KType::of_kind(KKind::Signature)),
            "Any" => Some(KType::ANY),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
