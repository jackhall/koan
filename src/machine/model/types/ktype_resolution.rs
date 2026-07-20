//! Surface-name and `TypeIdentifier` → `KType` elaboration.
//!
//! Join (least upper bound) and union canonicalization live on
//! [`TypeRegistry`](super::registry::TypeRegistry), which is where interning happens.

use super::kkind::KKind;
use super::ktype::KType;
use super::registry::TypeRegistry;
use crate::machine::model::ast::TypeIdentifier;

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

    /// Lower a parser `TypeIdentifier` into a `KType` against the builtin table only — no
    /// scope-aware resolver. The single entry point onto the [`KType::from_name`]
    /// builtin-table fallback: both the bind-time scopeless caller and the scope-aware
    /// [`elaborate_type_identifier`](crate::machine::model::types::elaborate_type_identifier)
    /// route their builtin fallback through here. Unknown names surface as `Err(_)`.
    pub fn from_type_identifier(
        t: &TypeIdentifier,
        _types: &TypeRegistry,
    ) -> Result<KType, String> {
        KType::from_name(t.as_str()).ok_or_else(|| format!("unknown type name `{}`", t.as_str()))
    }
}

#[cfg(test)]
mod tests;
