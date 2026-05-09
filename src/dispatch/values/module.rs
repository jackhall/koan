//! `Module` and `Signature` — first-class module values produced by the `MODULE` and `SIG`
//! builtins. See [design/module-system.md](../../../design/module-system.md).
//!
//! **Terminology — "module-signature" vs "expression-signature".** `Signature` here is the
//! **module-signature** type (`SIG`-declared) — an interface a module can be ascribed to
//! via `:|` / `:!`. The **expression-signature** machinery (`ExpressionSignature`,
//! `Argument`, `SignatureElement`) lives in [`crate::dispatch::types::signature`]. The two
//! are distinct concepts; do not conflate.
//!
//! Lifetime erasure on the scope pointer follows the same pattern as
//! [`KFunction`](crate::dispatch::kfunction::KFunction) and
//! [`RuntimeArena`](crate::dispatch::runtime::arena::RuntimeArena); per-site SAFETY blocks
//! sit at the `unsafe` `as_ref()` calls below.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::dispatch::runtime::{KError, KErrorKind, Scope};

use super::super::types::KType;
use super::KObject;

/// First-class module value. `path` is the lexical-source label (`"IntOrd"`,
/// `"Outer.Inner"`); `type_members` maps the module's abstract type names to the `KType`
/// they currently expose (e.g. `Foo.Type` resolving to a `KType::ModuleType`).
pub struct Module<'a> {
    pub path: String,
    child_scope_ptr: *const Scope<'static>,
    /// `RefCell` because opaque-ascription installs entries after the surrounding `KObject`
    /// is alloc'd. `Module` is arena-pinned and never moved, so a `&'a Module<'a>` borrow
    /// stays valid alongside interior mutation.
    pub type_members: RefCell<HashMap<String, KType>>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> Module<'a> {
    pub fn new(path: String, child_scope: &'a Scope<'a>) -> Self {
        // `Scope` is invariant in `'a`; the through-`'static` cast is required to match
        // the field type. Clippy reports it as redundant — false positive.
        #[allow(clippy::unnecessary_cast)]
        let child_scope_ptr = child_scope as *const Scope<'_> as *const Scope<'static>;
        Self {
            path,
            child_scope_ptr,
            type_members: RefCell::new(HashMap::new()),
            _marker: std::marker::PhantomData,
        }
    }

    /// Re-attach `'a` to the stored scope pointer. SAFETY: the underlying scope is
    /// arena-allocated; the arena outlives every `&Module<'a>` by construction.
    pub fn child_scope(&self) -> &'a Scope<'a> {
        unsafe {
            std::mem::transmute::<&Scope<'static>, &'a Scope<'a>>(&*self.child_scope_ptr)
        }
    }

    /// Stable identity used to seed `KType::ModuleType { scope_id, .. }`. Two distinct
    /// opaque ascriptions of the same source module mint distinct `ModuleType`s because
    /// each ascription allocates a fresh child scope.
    pub fn scope_id(&self) -> usize {
        self.child_scope_ptr as usize
    }
}

/// First-class signature (module type) value. Holds the raw declaration scope so
/// `:|` / `:!` can iterate the declared abstract types and operation signatures at
/// ascription time.
pub struct Signature<'a> {
    pub path: String,
    decl_scope_ptr: *const Scope<'static>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> Signature<'a> {
    pub fn new(path: String, decl_scope: &'a Scope<'a>) -> Self {
        // See `Module::new`.
        #[allow(clippy::unnecessary_cast)]
        let decl_scope_ptr = decl_scope as *const Scope<'_> as *const Scope<'static>;
        Self {
            path,
            decl_scope_ptr,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn decl_scope(&self) -> &'a Scope<'a> {
        unsafe {
            std::mem::transmute::<&Scope<'static>, &'a Scope<'a>>(&*self.decl_scope_ptr)
        }
    }
}

/// Resolve a `KObject` slot to a borrowed `&Module`. Accepts either a `KObject::KModule`
/// or a `KObject::TypeExprValue` token that names a module bound in `scope` (module names
/// classify as Type tokens at the surface, e.g. `IntOrd :| OrderedSig`).
///
/// `arg_name` is the surface argument label threaded into any produced `TypeMismatch`.
pub(crate) fn resolve_module<'a>(
    scope: &'a Scope<'a>,
    obj: &KObject<'a>,
    arg_name: &str,
) -> Result<&'a Module<'a>, KError> {
    if let Some(m) = obj.as_module() {
        return Ok(m);
    }
    if let Some(t) = obj.as_type_expr() {
        return match scope.lookup(&t.name) {
            Some(found) => found.as_module().ok_or_else(|| {
                KError::new(KErrorKind::TypeMismatch {
                    arg: arg_name.to_string(),
                    expected: "Module".to_string(),
                    got: found.ktype().name(),
                })
            }),
            None => Err(KError::new(KErrorKind::UnboundName(t.name.clone()))),
        };
    }
    Err(KError::new(KErrorKind::TypeMismatch {
        arg: arg_name.to_string(),
        expected: "Module".to_string(),
        got: obj.ktype().name(),
    }))
}

/// Symmetric to [`resolve_module`] for `&Signature`.
pub(crate) fn resolve_signature<'a>(
    scope: &'a Scope<'a>,
    obj: &KObject<'a>,
    arg_name: &str,
) -> Result<&'a Signature<'a>, KError> {
    if let Some(s) = obj.as_signature() {
        return Ok(s);
    }
    if let Some(t) = obj.as_type_expr() {
        return match scope.lookup(&t.name) {
            Some(found) => found.as_signature().ok_or_else(|| {
                KError::new(KErrorKind::TypeMismatch {
                    arg: arg_name.to_string(),
                    expected: "Signature".to_string(),
                    got: found.ktype().name(),
                })
            }),
            None => Err(KError::new(KErrorKind::UnboundName(t.name.clone()))),
        };
    }
    Err(KError::new(KErrorKind::TypeMismatch {
        arg: arg_name.to_string(),
        expected: "Signature".to_string(),
        got: obj.ktype().name(),
    }))
}

#[cfg(test)]
mod tests {
    //! Targeted Miri coverage for the `Module` / `Signature` unsafe sites: the
    //! `*const Scope<'static>` lifetime-erasure transmutes and `type_members` `RefCell`
    //! mutation under a held `&'a Module<'a>` borrow. Each shape is exercised in
    //! isolation so a regression attributes to a single site rather than an end-to-end run.
    //! See [`design/memory-model.md`](../../../design/memory-model.md).
    use super::*;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::types::KType;
    use std::io::sink;
    use std::ptr;
    #[test]
    fn module_child_scope_transmute_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let module = arena.alloc_module(Module::new("Test".into(), scope));
        let recovered = module.child_scope();
        assert!(ptr::eq(recovered, scope));
        // Re-borrow after a sibling alloc — tree borrows is sensitive to interleaved
        // mutation under live shared borrows.
        let _other = arena.alloc_object(crate::dispatch::values::KObject::Number(1.0));
        let recovered2 = module.child_scope();
        assert!(ptr::eq(recovered2, scope));
    }

    /// Covered independently of the module path because `Signature` lives on a different
    /// sub-arena (`signatures`) — a regression in `alloc_signature` or `decl_scope` must
    /// surface without the module path masking it.
    #[test]
    fn signature_decl_scope_transmute_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
        let recovered = sig.decl_scope();
        assert!(ptr::eq(recovered, scope));
        let _other = arena.alloc_object(crate::dispatch::values::KObject::Number(1.0));
        let recovered2 = sig.decl_scope();
        assert!(ptr::eq(recovered2, scope));
    }

    /// Opaque ascription mutates `type_members` after the surrounding `KObject` is alloc'd,
    /// so the `&'a Module<'a>` borrow is live across the `borrow_mut` + insert. Tree
    /// borrows is strict about interior mutation under a live shared borrow.
    #[test]
    fn module_type_members_refcell_mutation_with_held_module_ref() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let module = arena.alloc_module(Module::new("M".into(), scope));
        let scope_id = module.scope_id();
        {
            let mut tm = module.type_members.borrow_mut();
            tm.insert(
                "Type".into(),
                KType::ModuleType { scope_id, name: "Type".into() },
            );
        }
        let bound = module.type_members.borrow().get("Type").cloned();
        assert!(matches!(
            &bound,
            Some(KType::ModuleType { scope_id: id, name }) if *id == scope_id && name == "Type"
        ));
    }
}
