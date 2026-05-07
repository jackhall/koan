//! Runtime values: the universal `KObject` enum, dict-key wrapper `KKey`, and the
//! construction primitives for product (`struct_value`) and sum (`tagged_union`) types.
//! `named_pairs` is the shared parser used by both construction primitives and first-class
//! function calls.
//!
//! Construction primitives stay at module scope (`pub mod struct_value` / `pub mod
//! tagged_union`) because callers reach for the module's `apply` / `register` functions
//! rather than for a single re-exported item.
//!
//! [`dispatch_constructor`] is the shared entry point both `type_call` and `call_by_name`
//! use to route a resolved verb-object to the right construction primitive — collapsing the
//! `TaggedUnionType` / `StructType` branch that used to live duplicated in those two
//! builtins.

mod kkey;
mod kobject;
mod module;
mod named_pairs;
pub mod struct_value;
pub mod tagged_union;

pub use kkey::KKey;
pub use kobject::KObject;
pub use module::{Module, Signature};
pub use named_pairs::parse_named_value_pairs;

use crate::dispatch::kfunction::BodyResult;
use crate::parse::kexpression::ExpressionPart;

/// Route a resolved verb-object to its construction primitive's `apply` function. Returns
/// `Some(BodyResult)` when `verb_obj` is a constructible type (`TaggedUnionType` or
/// `StructType`); returns `None` when it isn't, so the caller can produce its own
/// not-a-constructor error message — `type_call` says "expected Type", `call_by_name` says
/// "expected KFunction or Type". Single growth point for stage 3 (first-class modules),
/// which will add a `ModuleType` arm here.
pub fn dispatch_constructor<'a>(
    verb_obj: &'a KObject<'a>,
    args_parts: Vec<ExpressionPart<'a>>,
) -> Option<BodyResult<'a>> {
    match verb_obj {
        KObject::TaggedUnionType(_) => Some(tagged_union::apply(verb_obj, args_parts)),
        KObject::StructType { .. } => Some(struct_value::apply(verb_obj, args_parts)),
        _ => None,
    }
}
