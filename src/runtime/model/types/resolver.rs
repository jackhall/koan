//! Pluggable type-name resolution. Consulted before the builtin `KType::from_name` table
//! so a module-local binding can shadow a builtin of the same name.

use crate::runtime::machine::core::Scope;
use crate::runtime::model::values::KObject;

use super::ktype::KType;

pub trait TypeResolver {
    fn resolve(&self, name: &str) -> Option<KType>;
}

pub struct NoopResolver;

impl TypeResolver for NoopResolver {
    fn resolve(&self, _name: &str) -> Option<KType> {
        None
    }
}

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
            // Shadowing applies only at the top-level lookup, not inside the
            // already-resolved `TypeExpr`'s parameters.
            KObject::TypeExprValue(t) => KType::from_type_expr(t, &NoopResolver).ok(),
            // SIG names lower to `SignatureBound` so a FN parameter typed `E: OrderedSig`
            // gets a per-sig admissibility slot rather than the catch-all `KType::Module`.
            // `sig_id` is the declaring `Signature`'s stable address; the dispatcher
            // checks it against the candidate module's `compatible_sigs` set.
            KObject::KSignature(s) => Some(KType::SignatureBound {
                sig_id: s.sig_id(),
                sig_path: s.path.clone(),
            }),
            _ => None,
        }
    }
}
