pub mod ast;
pub(crate) mod binder;
pub(crate) mod labels;
pub mod operators;
pub(crate) mod registries;
pub(crate) mod types;
pub(crate) mod values;

pub use labels::{
    BinderSymbol, KeywordSymbol, LabelInterner, Symbol, TypeSymbol, ValueSymbol, is_type_name,
};
pub use operators::{
    FoldDirection, OperatorGroup, OperatorGroupFamily, ReductionMode, binary_key, probe_key,
    unary_key,
};
pub use registries::RunRegistries;
pub use types::TypeRegistry;
pub use types::render_label;
pub use types::{
    AnnouncedData, AnnouncedMember, AnnouncedWindow, Argument, DeclWindow, DeferredReturn,
    DeferredReturnSurface, DispatchToken, DispatchTokenElement, ExpressionSignature, KKind, KType,
    NodeSchema, Parseable, PendingMember, Record, RecursiveGroupWindow, RelativeSchema, ReturnType,
    SealedAnnounced, SealedGroup, SignatureDraft, SignatureElement, TypeNode, UntypedElement,
    UntypedKey, WindowView, is_keyword_token,
};
pub use types::{
    StoredDispatchTokenElement, StoredElement, StoredKeyProbe, UntypedKeyProbe,
    most_specific_ktype, owned_untyped_key, restore_stored_key, store_untyped_key,
};
pub use values::{
    Carried, ContainerSubstrate, Held, KKey, KObject, PartedCell, Scalar, ValueEqualityError,
};

pub(crate) use ast::{
    DispatchShape, ExpressionPart, KExpression, KLiteral, Part, PartClass, ProgramExpression,
    TypeIdentifier, WorkingExpression, WorkingPart, classify_dispatch_shape,
};
pub use binder::{BindKind, BinderBucketFn, BinderNameFn, BinderSurface};
pub(crate) use binder::{OpArity, op_declaration_arity};
pub(crate) use binder::{StoredBinderKey, symbol_from_parts, symbol_from_quote_body};
pub(crate) use binder::{TypeDeclarationSurface, announced_type_declaration};
pub(crate) use types::{
    Elaborator, FieldListContext, FieldListOutcome, FieldNameKind, FieldParts, ResultFeed,
    SealOutcome, SigSchema, TypeResolution, constructor_param_names, declarator_window,
    elaborate_type_identifier, finalize_nominal_member, pair_list_names,
    parse_typed_field_list_via_elaborator, rewrite_window_refs, seal_writes,
    unsaturated_constructor_message,
};
/// Re-exported for the ascription builtin; `TypeDigest` also for the recursive-type test units.
pub(crate) use types::{TypeDigest, sig_subtype, substitute_sig_members};
pub(crate) use values::{
    CarriedFamily, Module, ModuleDraft, NamedPairs, RegionEscape, copy_or_pin,
    relocate_object_into, retains_home,
};
