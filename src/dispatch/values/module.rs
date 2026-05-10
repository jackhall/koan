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
    /// Sigs this module shape-checks against. Populated by `:|` and `:!` at ascription
    /// time via [`Module::mark_satisfies`]. `accepts_part` for `KType::SignatureBound {
    /// sig_id }` is an O(1) membership check against this set. `RefCell` because
    /// ascription writes after the surrounding `KObject::KModule` is already alloc'd —
    /// same shape as `type_members`.
    pub compatible_sigs: RefCell<Vec<usize>>,
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
            compatible_sigs: RefCell::new(Vec::new()),
            _marker: std::marker::PhantomData,
        }
    }

    /// Record that this module shape-checks against `sig_id`. Routed through one named
    /// method (rather than open-coded `compatible_sigs.borrow_mut().push(...)` at each
    /// ascription site) so future ascription paths are easy to grep for, and so the
    /// idempotency check sits in one place — re-ascribing the same module to the same sig
    /// (e.g. `(View :| OrderedSig)` after `(View :! OrderedSig)`) doesn't double-insert.
    pub fn mark_satisfies(&self, sig_id: usize) {
        let mut s = self.compatible_sigs.borrow_mut();
        if !s.contains(&sig_id) {
            s.push(sig_id);
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

    /// Stable identity used to seed `KType::SignatureBound { sig_id, .. }`. Mirrors
    /// `Module::scope_id` — the arena pins the `Signature` for the run, addresses are
    /// stable and unique, and two `SIG Foo = (...)` declarations in the same scope
    /// already error (`Rebind`), so distinct `Signature` values always have distinct ids.
    pub fn sig_id(&self) -> usize {
        self.decl_scope_ptr as usize
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

    /// Module-system stage 2 (functor slice). Minimal-shape mirror of
    /// [`crate::execute::lift::lift_kobject`]'s `KModule` arm: build a `Module` whose
    /// `child_scope` lives in a `CallArena`, lift it against the dying frame, and assert
    /// the lifted result carries the arena anchor. Pins the unsafe site behind functor
    /// execution end-to-end.
    #[test]
    fn functor_per_call_module_lifts_correctly() {
        use crate::dispatch::kfunction::{Body, KFunction};
        use crate::dispatch::runtime::{CallArena, RuntimeArena as RA};
        use crate::dispatch::types::{ExpressionSignature, KType, SignatureElement};
        use crate::dispatch::values::KObject;
        use crate::execute::lift::lift_kobject_for_test;
        use std::rc::Rc;

        let outer_arena = RuntimeArena::new();
        let outer_scope = default_scope(&outer_arena, Box::new(sink()));
        let frame: Rc<CallArena> = CallArena::new(outer_scope, None);

        // Borrow into the per-call arena via raw-pointer roundtrip so the borrow doesn't
        // outlive `frame` for the borrow-checker (the SAFETY invariant on `CallArena` —
        // arena heap address is stable for the Rc's life — backs this).
        let arena_ptr: *const RA = frame.arena();
        let inner_arena: &RA = unsafe { &*arena_ptr };

        // Defeat `functions_is_empty()`'s fast path so the slow lift path runs.
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: KType::Null,
                elements: vec![SignatureElement::Keyword("__SLOW__".into())],
            },
            Body::Builtin(|s, _, _| {
                crate::dispatch::kfunction::BodyResult::Value(s.arena.alloc_object(KObject::Null))
            }),
            frame.scope(),
        );
        let _ = inner_arena.alloc_function(kf);

        // Module's `child_scope` lives in `inner_arena` — exactly the shape a functor
        // body's `MODULE Result = (...)` produces. Lift must observe the arena match.
        let inner_scope = inner_arena.alloc_scope(
            crate::dispatch::runtime::Scope::child_under_named(frame.scope(), "Inner".into()),
        );
        let module = inner_arena.alloc_module(Module::new("Inner".into(), inner_scope));
        let m_obj = KObject::KModule(module, None);

        let strong_before = Rc::strong_count(&frame);
        let lifted = lift_kobject_for_test(&m_obj, &frame);
        match &lifted {
            KObject::KModule(_, anchor) => assert!(
                anchor.is_some(),
                "KModule whose child scope lives in the dying arena must lift with frame=Some(rc)",
            ),
            other => panic!("expected lifted KModule, got {:?}", other.ktype()),
        }
        assert_eq!(
            Rc::strong_count(&frame),
            strong_before + 1,
            "lifting a per-frame module must clone the dying frame's Rc once",
        );
        // Drop borrowers before `frame` so arena teardown order is well-defined.
        drop(lifted);
        drop(m_obj);
    }
}
