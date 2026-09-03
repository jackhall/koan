pub mod ast;
pub(crate) mod binder;
pub(crate) mod close_inference;
pub(crate) mod key_spec;
pub(crate) mod labels;
pub(crate) mod lazy_slots;
pub(crate) mod miss_diagnostics;
pub mod operators;
pub(crate) mod registries;
pub(crate) mod types;
pub(crate) mod values;

#[cfg(feature = "alloc-count")]
pub use labels::symbols_minted;
pub use labels::{
    BinderSymbol, ClassifiedSymbol, KeywordSymbol, LabelInterner, StaticName, Symbol, TypeSymbol,
    ValueSymbol, WILDCARD, is_type_name, snake_case_identifier, wrong_binder_class,
};
pub use operators::{
    FoldDirection, OperatorGroup, OperatorGroupFamily, ReductionMode, binary_key, unary_key,
};
pub use registries::RunRegistries;
pub(crate) use types::IdentityBuildHasher;
pub use types::TypeRegistry;
pub use types::builtin_types;
pub use types::most_specific_ktype;
pub use types::{
    AnnouncedData, AnnouncedMember, AnnouncedWindow, Argument, DeclWindow, DeferredReturn,
    DeferredReturnSurface, DispatchToken, DispatchTokenElement, ExpressionSignature, KKind, KType,
    KeyElement, NodeSchema, Parseable, PendingMember, Record, RecursiveGroupWindow, RelativeSchema,
    ReturnType, SealedAnnounced, SealedGroup, SignatureDraft, SignatureElement, TypeNode,
    UntypedKey, WindowView, is_keyword_token,
};
pub use types::{
    CaptureShape, CaptureShapes, capture_footprint, capture_shape_of, carrier_union_error,
    is_exact_carrier,
};
pub use types::{display_label, render_label};
pub(crate) use types::{render_untyped_key, summarize_dispatch, untyped_key_of};
pub use values::{
    Carried, ContainerSubstrate, Held, KKey, KObject, PartedCell, Scalar, ValueEqualityError,
};

pub(crate) use ast::{
    DispatchShape, ExpressionPart, KExpression, KLiteral, Part, PartClass, ProgramExpression,
    ProgramNode, WorkingExpression, WorkingPart, classify_dispatch_shape,
};
pub(crate) use binder::MACHINE_BINDERS;
pub(crate) use binder::admit_bare_type_slots;
pub(crate) use binder::announce_type_members;
pub(crate) use binder::signature::{SignaturePosition, SignatureScan};
pub use binder::{BindKind, BinderBucketFn, BinderNameFn, BinderSurface};
pub(crate) use binder::{OpArity, op_declaration_arity};
pub(crate) use binder::{StoredBinderKey, symbol_from_parts, symbol_from_quote_body};
pub use close_inference::DynamicNameForm;
pub(crate) use close_inference::infer_close_captures;
pub(crate) use miss_diagnostics::{diagnose_miss, key_is_reserved};
pub(crate) use types::{
    Elaborator, FieldListContext, FieldListOutcome, FieldNameKind, FieldParts, ResultFeed,
    SealOutcome, SigSchema, TypeMemberMap, TypeResolution, constructor_param_names,
    declarator_window, elaborate_type_identifier, finalize_nominal_member, pair_list_names,
    parse_typed_field_list_via_elaborator, rewrite_window_refs, seal_writes, type_name_miss,
    unsaturated_constructor_message,
};
/// Re-exported for the ascription builtin; `TypeDigest` also for the recursive-type test units.
pub(crate) use types::{TypeDigest, sig_subtype, substitute_sig_members};
pub(crate) use values::{
    CarriedFamily, Module, ModuleDraft, NamedPairs, RegionEscape, copy_or_pin,
    copy_or_pin_callable, object_copy_cost, relocate_object_into, retains_home,
};
