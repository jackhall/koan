//! Type system and dispatch shape: the `KType` tag, function signatures, and the traits
//! every language object implements. Bottom of the dispatch dependency stack — `values`
//! and `runtime` build on it.

mod ktraits;
mod ktype;
mod ktype_predicates;
mod ktype_resolution;
mod resolver;
mod signature;
mod typed_field_list;

pub use ktraits::{Parseable, Serializable};
pub use ktype::KType;
#[allow(unused_imports)]
pub use resolver::{NoopResolver, ScopeResolver, TypeResolver};
pub use signature::{
    Argument, ExpressionSignature, SignatureElement, UntypedElement, UntypedKey, is_keyword_token,
};
#[allow(unused_imports)]
pub use signature::Specificity;
pub use typed_field_list::parse_typed_field_list;
