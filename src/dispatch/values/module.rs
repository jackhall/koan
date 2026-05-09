//! `Module` and `Signature` — first-class module values produced by the `MODULE` and `SIG`
//! builtins. See [design/module-system.md](../../../design/module-system.md) for the
//! cross-cutting design.
//!
//! **Terminology — "module-signature" vs "expression-signature".** `Signature` here is the
//! **module-signature** type (`SIG`-declared) — an interface a module can be ascribed to
//! via `:|` / `:!`. The **expression-signature** machinery — the FN-parameter-list type used
//! by dispatch (`ExpressionSignature`, `Argument`, `SignatureElement`) — lives in
//! [`crate::dispatch::types::signature`]. The two are distinct concepts; do not conflate.
//!
//! A `Module` bundles a child `Scope` (where the body's `LET`/`FN` bindings landed during
//! evaluation) with a textual `path` and a per-module type-members table. The path is the
//! lexical-source label (`"IntOrd"`, `"Outer.Inner"`); the type-members table maps the
//! module's abstract type names (`"Type"`) to the `KType` they currently expose. Opaquely-
//! ascribed modules carry a fresh `KType::ModuleType { scope_id, name }` value here, and
//! the `scope_id` is the address of the *new* (ascription-result) `Scope` so two distinct
//! opaque ascriptions of the same source module mint distinct types.
//!
//! Signatures are simpler: just a textual path, the captured scope holding the abstract
//! type declarations and operation signatures, and (for stage 1) no axioms — those land in
//! stage 4. Both shapes are arena-allocated so the same `'a` `KObject` lifetime contract
//! used for `KFunction` applies — `KModule(&'a Module<'a>)` keeps the value cheap to clone.
//!
//! **Lifetime erasure.** Like [`KFunction`](crate::dispatch::kfunction::KFunction), the
//! scope reference is held as `*const Scope<'static>` to keep `Module` invariant-friendly
//! across the `KObject` enum's `'a` parameter. The pointer is set from a `&'a Scope<'a>`
//! at construction and re-attached to the caller's `'a` via `child_scope()`. Same SAFETY
//! rationale as `KFunction::captured`: scopes are arena-allocated and never moved, the
//! arena outlives every reference into it.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::dispatch::runtime::{KError, KErrorKind, Scope};

use super::super::types::KType;
use super::KObject;

/// First-class module value. The `path` is the lexical-source label used by error messages
/// and `summarize()`; `child_scope_ptr` points into the same arena as the containing
/// `KObject` and is consulted by ATTR for member access; `type_members` records the module's
/// abstract type bindings — populated at opaque-ascription time and looked up by ATTR's
/// type-position fallback (e.g. `Foo.Type` resolving to a `KType::ModuleType`).
pub struct Module<'a> {
    pub path: String,
    child_scope_ptr: *const Scope<'static>,
    /// Per-module abstract-type bindings. Stored in a `RefCell` so opaque-ascription can
    /// install entries after the surrounding `KObject` has been alloc'd. `Module` is
    /// arena-pinned and never moved, so a `&'a Module<'a>` borrow stays valid alongside
    /// interior mutation.
    pub type_members: RefCell<HashMap<String, KType>>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> Module<'a> {
    pub fn new(path: String, child_scope: &'a Scope<'a>) -> Self {
        // `Scope` is invariant in `'a`; the through-`'static` cast is required to match
        // the `*const Scope<'static>` field type. Clippy reports it as redundant — wrong.
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

    /// Stable identity used to seed `KType::ModuleType { scope_id, .. }`. The address of
    /// the module's child scope is unique per module instance, so two distinct opaque
    /// ascriptions of the same source module mint distinct `ModuleType`s.
    pub fn scope_id(&self) -> usize {
        self.child_scope_ptr as usize
    }
}

/// First-class signature (module type) value. Stage 1 stores the raw declaration scope so
/// `:|` / `:!` can iterate the declared abstract types and operation signatures at
/// ascription time. Stage 4 will add axiom carriers here; until then the field set is
/// deliberately minimal.
pub struct Signature<'a> {
    pub path: String,
    decl_scope_ptr: *const Scope<'static>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> Signature<'a> {
    pub fn new(path: String, decl_scope: &'a Scope<'a>) -> Self {
        // See `Module::new` — `Scope` is invariant, the through-`'static` cast is required.
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

/// Resolve a `KObject` slot to a borrowed `&Module`. Accepts either an already-evaluated
/// `KObject::KModule` (when the lhs is a `Future(KModule)` from a sub-dispatch) or a
/// `KObject::TypeExprValue` token that names a module bound in `scope` (the surface case
/// where module names classify as Type tokens, e.g. `IntOrd :| OrderedSig`). Used by both
/// the ascription operators (`:|` / `:!`) and `MODULE_TYPE_OF`'s `m` slot — the dual-shape
/// pattern was duplicated in two builtin files before being lifted here.
///
/// `arg_name` is the surface argument label used in the produced `TypeMismatch` so error
/// messages stay byte-identical with the previous per-builtin helpers (`m` for both
/// consumers today; threading it keeps the API future-proof if a third site lands a
/// different label).
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

/// Symmetric to [`resolve_module`] for `&Signature`. Same dual-shape match
/// (`KObject::KSignature(_) | KObject::TypeExprValue(t)` with scope lookup) and same
/// `TypeMismatch` / `UnboundName` error shape. The shared callers are the ascription
/// operators' `s` slot — `MODULE_TYPE_OF` doesn't take a Signature today, but the helper
/// lives here because the ascription operators want a parallel API to `resolve_module`.
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
    //! Targeted Miri coverage for the post-stage-1 `Module` / `Signature` unsafe sites:
    //! the `*const Scope<'static>` lifetime-erasure transmutes and the `type_members`
    //! `RefCell` mutation under a held `&'a Module<'a>` borrow. Same convention as the
    //! arena.rs slate — fail when Miri reports UB, not on values.
    //!
    //! Per [`design/memory-model.md`](../../../design/memory-model.md), each shape is
    //! exercised in isolation so a regression in `Module::new` / `Module::child_scope` /
    //! `Signature::new` / `Signature::decl_scope` / `type_members.borrow_mut` shows up
    //! as a single attributable failure, not buried in a full end-to-end run.
    use super::*;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::types::KType;
    use std::io::sink;
    use std::ptr;
    /// `Module::new` casts `&'a Scope<'a>` through `*const Scope<'_>` to
    /// `*const Scope<'static>`; `child_scope()` re-attaches `'a` via transmute. The arena
    /// outlives the module by construction. Pin the round-trip down on its own — alloc
    /// the module into the arena, hand out a `&'a Module<'a>`, read its `child_scope()`
    /// back, and verify the recovered ref is pointer-identical to the input scope.
    #[test]
    fn module_child_scope_transmute_does_not_dangle() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let module = arena.alloc_module(Module::new("Test".into(), scope));
        let recovered = module.child_scope();
        assert!(ptr::eq(recovered, scope));
        // Re-borrow after a sibling alloc — typed-arena promises stable addresses, but
        // tree borrows is sensitive to interleaved mutation under live shared borrows.
        let _other = arena.alloc_object(crate::dispatch::values::KObject::Number(1.0));
        let recovered2 = module.child_scope();
        assert!(ptr::eq(recovered2, scope));
    }

    /// Symmetric to `module_child_scope_transmute_does_not_dangle`. `Signature` uses the
    /// same `*const Scope<'static>` shape; the slate covers it independently because the
    /// allocator lives on a different sub-arena (`signatures`) and a regression in either
    /// `alloc_signature`'s transmute or `Signature::decl_scope`'s re-attach must surface
    /// without the module path masking it.
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

    /// Opaque ascription mutates `type_members` *after* the surrounding `KObject` is
    /// alloc'd — the `&'a Module<'a>` borrow is already live when the borrow_mut + insert
    /// happens. Tree borrows is strict about interior mutation under a live shared
    /// borrow; pin the shape down. Read the value back through a fresh `borrow()` to
    /// verify the insert is observable.
    #[test]
    fn module_type_members_refcell_mutation_with_held_module_ref() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(sink()));
        let module = arena.alloc_module(Module::new("M".into(), scope));
        // Hold the `&Module` borrow live across the borrow_mut + insert + readback.
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
