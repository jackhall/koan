//! `TypeResolver` — pluggable type-name resolution. Stage 0 only ships the no-op resolver,
//! but `from_type_expr` / `parse_typed_field_list` already accept a `&dyn TypeResolver` so
//! stage 1 (first-class modules) can plug in a real one without touching every call site.
//!
//! Resolver-first precedence: when a name appears in user source (`x: SomeType`), the
//! resolver is consulted before the builtin `KType::from_name` table. A module-local
//! `Point` should be able to shadow a builtin `Point` without that decision needing to be
//! re-litigated at every callsite.

use super::ktype::KType;

/// Resolves a user-typed type name (`Point`, `Maybe`) to its registered `KType`. Returns
/// `None` if the name isn't bound in this resolver's scope, leaving callers free to fall
/// through to the builtin name table.
pub trait TypeResolver {
    fn resolve(&self, name: &str) -> Option<KType>;
}

/// Resolver that always returns `None`. Used everywhere stage 0 hasn't wired in a real
/// resolver yet — keeps the call sites' shape stable so stage 1's swap is pure plumbing.
pub struct NoopResolver;

impl TypeResolver for NoopResolver {
    fn resolve(&self, _name: &str) -> Option<KType> {
        None
    }
}
