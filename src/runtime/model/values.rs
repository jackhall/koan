//! Runtime values: the universal [`KObject`] enum, dict-key wrapper [`KKey`], and the
//! construction primitives for product ([`struct_value`]) and sum ([`tagged_union`]) types.
//! [`named_pairs`] is the shared parser used by both construction primitives and first-class
//! function calls.
//!
//! [`dispatch_constructor`] is the shared entry point routing a resolved verb-object to the
//! right construction primitive.

mod kkey;
mod kobject;
mod module;
mod named_pairs;
pub mod struct_value;
pub mod tagged_union;

pub use kkey::KKey;
pub use kobject::KObject;
pub use module::{Module, Signature};
pub(crate) use module::{resolve_module, resolve_signature};
pub use named_pairs::parse_named_value_pairs;

use crate::runtime::machine::kfunction::BodyResult;
use crate::ast::ExpressionPart;

/// Route a resolved verb-object to its construction primitive's `apply` function. Returns
/// `Some(BodyResult)` for `TaggedUnionType` / `StructType`; `None` otherwise.
/// Single growth point for stage 3 (first-class modules), which will add a `ModuleType` arm.
pub fn dispatch_constructor<'a>(
    verb_obj: &'a KObject<'a>,
    args_parts: Vec<ExpressionPart<'a>>,
) -> Option<BodyResult<'a>> {
    match verb_obj {
        KObject::TaggedUnionType { .. } => Some(tagged_union::apply(verb_obj, args_parts)),
        KObject::StructType { .. } => Some(struct_value::apply(verb_obj, args_parts)),
        _ => None,
    }
}
