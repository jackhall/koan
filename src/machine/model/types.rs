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
mod unify;

pub use ktraits::{Parseable, Serializable};
pub use ktype::{KType, UserTypeKind};
pub use resolver::{elaborate_type_expr, ElabResult, Elaborator};
pub use signature::{
    Argument, DeferredReturn, ExpressionSignature, ReturnType, SignatureElement, UntypedElement,
    UntypedKey, is_keyword_token,
};
#[allow(unused_imports)]
pub use signature::Specificity;
pub use typed_field_list::{parse_typed_field_list_via_elaborator, FieldListOutcome};
// Phase 5 generic-destructuring unifier — the reusable per-call type-parameter binding
// core. Exported as the seam the FN-signature deferral integration will consume; not yet
// wired into `invoke.rs` (see the roadmap item's remaining-work note), so the re-export is
// unused at the crate level for now.
#[allow(unused_imports)]
pub use unify::{unify_slot, UnifyResult};
