//! `TypeResolver` — pluggable type-name resolution. Two implementations: `NoopResolver`
//! (always `None`, used in tests / contexts with no surrounding scope) and `ScopeResolver`
//! (walks the surrounding `Scope::data` chain for `KObject::TypeExprValue` bindings, then
//! falls through to the builtin name table).
//!
//! Resolver-first precedence: when a name appears in user source (`x: SomeType`), the
//! resolver is consulted before the builtin `KType::from_name` table. A module-local
//! `Point` should be able to shadow a builtin `Point` without that decision needing to be
//! re-litigated at every callsite. The scope-aware resolver is what lands stage-2's
//! "type bindings live in `Scope::data`" property: a `LET MyType = (LIST_OF Number)`
//! binding makes `MyType` available as a type name in subsequent FN signatures.

use crate::dispatch::runtime::Scope;
use crate::dispatch::values::KObject;

use super::ktype::KType;

/// Resolves a user-typed type name (`Point`, `Maybe`) to its registered `KType`. Returns
/// `None` if the name isn't bound in this resolver's scope, leaving callers free to fall
/// through to the builtin name table.
pub trait TypeResolver {
    fn resolve(&self, name: &str) -> Option<KType>;
}

/// Resolver that always returns `None`. Used by the type-builtin tests and by any context
/// that has no surrounding scope to consult — falls through to the builtin name table.
pub struct NoopResolver;

impl TypeResolver for NoopResolver {
    fn resolve(&self, _name: &str) -> Option<KType> {
        None
    }
}

/// Resolver that walks `scope`'s `data` map (and its `outer` chain) for a binding whose
/// value is a `KObject::TypeExprValue`. The resolver lowers the bound `TypeExpr` via
/// `KType::from_type_expr` recursively (with a fresh `NoopResolver` for the recursive
/// step — name shadowing happens at the top-level lookup, not inside an already-resolved
/// `TypeExpr`'s parameters). Returns `None` for unbound names or for bindings of any
/// other variant; callers fall through to the builtin name table.
///
/// Stage 2 substrate: a user binding like `LET MyList = (LIST_OF Number)` puts a
/// `TypeExprValue` in scope, which this resolver promotes to `KType::List(Number)` for
/// any subsequent typed slot that names `MyList`.
pub struct ScopeResolver<'s, 'a> {
    pub scope: &'s Scope<'a>,
}

impl<'s, 'a> ScopeResolver<'s, 'a> {
    pub fn new(scope: &'s Scope<'a>) -> Self {
        Self { scope }
    }
}

impl<'s, 'a> TypeResolver for ScopeResolver<'s, 'a> {
    fn resolve(&self, name: &str) -> Option<KType> {
        let bound = self.scope.lookup(name)?;
        match bound {
            KObject::TypeExprValue(t) => KType::from_type_expr(t, &NoopResolver).ok(),
            _ => None,
        }
    }
}
