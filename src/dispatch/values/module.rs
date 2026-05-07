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

use crate::dispatch::runtime::Scope;

use super::super::types::KType;

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
