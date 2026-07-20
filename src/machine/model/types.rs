//! Type system and dispatch shape: the `KType` tag, function signatures, and the traits
//! every language object implements. Bottom of the dispatch dependency stack — `values`
//! and `runtime` build on it.

mod kkind;
mod ktraits;
mod ktype;
mod ktype_predicates;
mod ktype_resolution;
mod node;
mod record;
mod recursive_group_window;
pub(crate) mod registry;
mod resolver;
mod sig_schema;
mod signature;
mod type_digest;
mod typed_field_list;

pub use kkind::KKind;
pub use ktraits::Parseable;
pub use ktype::KType;
pub use node::{NodeSchema, TypeNode};
pub use record::Record;
pub use recursive_group_window::{
    PendingMember, RecursiveGroupWindow, RelativeSchema, SealedGroup,
};
pub(crate) use registry::Relation;
pub use registry::TypeRegistry;
pub use resolver::{
    declarator_window, elaborate_type_identifier, finalize_nominal_member, Elaborator, SealOutcome,
    TypeResolution,
};
pub use sig_schema::{
    constructor_param_names, sig_subtype, substitute_sig_members, unsaturated_constructor_message,
    SigSchema,
};
#[allow(unused_imports)]
pub use signature::Specificity;
pub use signature::{
    is_keyword_token, Argument, DeferredReturn, DeferredReturnSurface, ExpressionSignature,
    ReturnType, SignatureElement, UntypedElement, UntypedKey,
};
pub(crate) use type_digest::{empty_schema_digest, TypeDigest};
pub use typed_field_list::{
    pair_list_names, parse_typed_field_list_via_elaborator, FieldListContext, FieldListOutcome,
    FieldNameKind, ResultFeed,
};
