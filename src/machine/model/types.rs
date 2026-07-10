//! Type system and dispatch shape: the `KType` tag, function signatures, and the traits
//! every language object implements. Bottom of the dispatch dependency stack — `values`
//! and `runtime` build on it.

mod kkind;
mod ktraits;
mod ktype;
mod ktype_predicates;
mod ktype_resolution;
mod record;
mod recursive_set;
mod resolver;
mod signature;
mod typed_field_list;

pub use kkind::KKind;
pub use ktraits::{Parseable, Serializable};
pub use ktype::{AbstractSource, KType};
pub use record::Record;
pub use recursive_set::{
    seal_recursive_refs, seal_union_refs, NominalMember, NominalSchema, ProjectedSchema,
    RecursiveSet,
};
pub use resolver::{
    elaborate_type_identifier, finalize_nominal_member, Elaborator, SchemaSealResult, SealOutcome,
    TypeResolution,
};
#[allow(unused_imports)]
pub use signature::Specificity;
pub use signature::{
    is_keyword_token, Argument, DeferredReturn, DeferredReturnSurface, ExpressionSignature,
    ReturnType, SignatureElement, UntypedElement, UntypedKey,
};
pub use typed_field_list::{
    parse_typed_field_list_via_elaborator, FieldListOutcome, FieldNameKind, ResultFeed,
};
