//! Type system and dispatch shape: the `KType` tag, function signatures, and the traits
//! every language object implements. This is the bottom of the dispatch dependency stack —
//! `values` and `runtime` build on it.
//!
//! Internal layout (private modules) is not part of the public API; callers reach for the
//! re-exports below.

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
    Argument, ExpressionSignature, SignatureElement, Specificity, UntypedElement, UntypedKey,
    is_keyword_token,
};
pub use typed_field_list::parse_typed_field_list;
