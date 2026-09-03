//! The builtin type vocabulary, declared once in Rust source.
//!
//! Each name is a [`StaticName`], so its symbol is minted at first read and loaded thereafter:
//! seeding a second run's root re-registers the same twelve names without hashing a single
//! spelling. [`builtin_types`] pairs each name with the fixed handle it lowers to, and is the one
//! table both registration and [`KType::from_symbol`](KType::from_symbol) read.

use super::kkind::KKind;
use super::ktype::KType;
use crate::machine::model::labels::{StaticName, TypeSymbol};

pub static NUMBER: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Number");
pub static STR: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Str");
pub static BOOL: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Bool");
pub static NULL: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Null");
pub static LIST: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "List");
pub static DICT: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Dict");
pub static KEXPRESSION: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "KExpression");
pub static TYPE: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Type");
pub static MODULE: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Module");
pub static SIGNATURE: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Signature");
pub static ANY: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Any");
pub static NEVER: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Never");

/// Every builtin type name beside the handle it lowers to, in registration order.
///
/// A bare `List` or `Dict` names its fully-general instance, and `Module` names the empty
/// signature — the surface spelling admits no parameters, so the handle it stands for is fixed.
/// `Never` names the lattice bottom, so a slot written `:Never` is legal and admits nothing.
pub fn builtin_types() -> [(&'static StaticName<TypeSymbol>, KType); 12] {
    [
        (&NUMBER, KType::NUMBER),
        (&STR, KType::STR),
        (&BOOL, KType::BOOL),
        (&NULL, KType::NULL),
        (&LIST, KType::LIST_OF_ANY),
        (&DICT, KType::DICT_ANY_ANY),
        (&KEXPRESSION, KType::KEXPRESSION),
        (&TYPE, KType::of_kind(KKind::AnyType)),
        (&MODULE, KType::EMPTY_SIGNATURE),
        (&SIGNATURE, KType::of_kind(KKind::Signature)),
        (&ANY, KType::ANY),
        (&NEVER, KType::NEVER),
    ]
}
