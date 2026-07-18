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
pub(crate) mod registry;
mod resolver;
mod sig_schema;
mod signature;
mod type_digest;
mod typed_field_list;

pub use kkind::KKind;
pub use ktraits::Parseable;
pub use ktype::{KType, SigSource};
pub use record::Record;
pub(crate) use recursive_set::same_nominal;
pub use recursive_set::{
    seal_recursive_refs, seal_union_refs, NominalMember, NominalSchema, ProjectedSchema,
    RecursiveSet,
};
pub(crate) use registry::Relation;
pub use registry::TypeRegistry;
pub use resolver::{
    elaborate_type_identifier, finalize_nominal_member, Elaborator, SchemaSealResult, SealOutcome,
    TypeResolution,
};
pub use sig_schema::{sig_subtype, substitute_sig_members, SigSchema};
#[allow(unused_imports)]
pub use signature::Specificity;
pub use signature::{
    is_keyword_token, Argument, DeferredReturn, DeferredReturnSurface, ExpressionSignature,
    ReturnType, SignatureElement, UntypedElement, UntypedKey,
};
pub(crate) use type_digest::{schema_content_digest, signature_digest, TypeDigest};
pub use typed_field_list::{
    parse_typed_field_list_via_elaborator, FieldListOutcome, FieldNameKind, ResultFeed,
};
