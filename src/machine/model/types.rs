//! Type system and dispatch shape: the `KType` tag, function signatures, and the traits
//! every language object implements. Bottom of the dispatch dependency stack — `values`
//! and `runtime` build on it.

mod ktraits;
mod ktype;
mod ktype_predicates;
mod ktype_resolution;
mod record;
mod resolver;
mod signature;
mod typed_field_list;

pub use ktraits::{Parseable, Serializable};
pub use ktype::{AbstractSource, KType, UserTypeKind};
pub use record::Record;
pub use resolver::{elaborate_type_expr, ElabResult, Elaborator};
#[allow(unused_imports)]
pub use signature::Specificity;
pub use signature::{
    is_keyword_token, Argument, DeferredReturn, DeferredReturnSurface, ExpressionSignature,
    ReturnType, SignatureElement, UntypedElement, UntypedKey,
};
pub use typed_field_list::{
    parse_typed_field_list_via_elaborator, FieldListOutcome, FieldNameKind,
};
