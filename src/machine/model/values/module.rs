//! `Module` and `Signature` — first-class module values produced by the `MODULE` and `SIG`
//! builtins. See [design/typing/modules.md](../../../../design/typing/modules.md).
//!
//! **Terminology — "module-signature" vs "expression-signature".** `Signature` here is the
//! **module-signature** type (`SIG`-declared) — an interface a module can be ascribed to
//! via `:|` / `:!`. The **expression-signature** machinery (`ExpressionSignature`,
//! `Argument`, `SignatureElement`) lives in [`crate::machine::model::types::signature`]. The two
//! are distinct concepts; do not conflate.
//!
//! Lifetime erasure on the scope pointer routes through
//! [`ScopePtr`](crate::machine::core::scope_ptr::ScopePtr), shared with
//! [`KFunction`](crate::machine::core::kfunction::KFunction) and
//! [`CallArena`](crate::machine::core::arena::CallArena). The branded `ScopePtr<'a>` makes
//! `child_scope` / `decl_scope` safe re-attaches; the irreducible `unsafe` re-attach lives at
//! `CallArena`.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::machine::core::{Scope, ScopeId, ScopePtr};

use super::super::types::KType;

/// First-class module value. `path` is the lexical-source label (`"IntOrd"`,
/// `"Outer.Inner"`); `type_members` maps the module's abstract type names to the `KType`
/// they currently expose. Opaque-ascription members mint `KType::AbstractType { source:
/// Module(self), name }`; the module value itself rides `KType::Module { module, frame }`
/// in the surrounding `KObject::KTypeValue` (the two are distinguished by `KType` variant —
/// `AbstractType` vs `Module`).
pub struct Module<'a> {
    pub path: String,
    child_scope_ptr: ScopePtr<'a>,
    /// `RefCell` because opaque-ascription installs entries after the surrounding `KObject`
    /// is alloc'd. `Module` is arena-pinned and never moved, so a `&'a Module<'a>` borrow
    /// stays valid alongside interior mutation.
    pub type_members: RefCell<HashMap<String, KType<'a>>>,
    /// VAL-slot name → the per-call abstract `KType` an opaque ascription minted for the
    /// slot's SIG-declared type. ATTR re-tags a value-side slot read with this identity so
    /// `(int_ord.zero)` reads as the abstract `Type`, not the underlying concrete value.
    /// Empty for unascribed and transparently-ascribed (`:!`) modules. Same `RefCell`
    /// rationale as `type_members` — populated after the surrounding `KObject` is alloc'd.
    pub slot_type_tags: RefCell<HashMap<String, KType<'a>>>,
    /// Sigs this module shape-checks against. `accepts_part` for a
    /// `KType::Signature { sig, .. }` slot is an O(1) `sig.sig_id()` membership check
    /// against this set. `RefCell` for the same reason as `type_members` — ascription
    /// writes after the surrounding `KObject::KModule` is already alloc'd.
    pub compatible_sigs: RefCell<Vec<ScopeId>>,
}

impl<'a> Module<'a> {
    pub fn new(path: String, child_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            child_scope_ptr: ScopePtr::erase(child_scope),
            type_members: RefCell::new(HashMap::new()),
            slot_type_tags: RefCell::new(HashMap::new()),
            compatible_sigs: RefCell::new(Vec::new()),
        }
    }

    /// Record that this module shape-checks against `sig_id`. Idempotent — re-ascribing
    /// (e.g. `(View :| OrderedSig)` after `(View :! OrderedSig)`) doesn't double-insert.
    pub fn mark_satisfies(&self, sig_id: ScopeId) {
        let mut s = self.compatible_sigs.borrow_mut();
        if !s.contains(&sig_id) {
            s.push(sig_id);
        }
    }

    /// Re-attach `'a` to the stored scope. The branded `child_scope_ptr` makes this a safe
    /// re-attach: it consumed a real `&'a Scope<'a>` at construction, and the arena outlives
    /// every `&Module<'a>` by construction.
    pub fn child_scope(&self) -> &'a Scope<'a> {
        self.child_scope_ptr.reattach()
    }

    /// Stable identity keyed by `KType::Module` equality (and recorded on per-call abstract
    /// members minted from this module). Two distinct opaque ascriptions of the same source
    /// module compare distinct because each allocates a fresh child scope (and thus a fresh
    /// `ScopeId`).
    pub fn scope_id(&self) -> ScopeId {
        self.child_scope().id
    }
}

/// First-class signature (module type) value. Holds the raw declaration scope so
/// `:|` / `:!` can iterate the declared abstract types and operation signatures at
/// ascription time.
pub struct Signature<'a> {
    pub path: String,
    /// Branded [`ScopePtr<'a>`]: `Scope<'a>` is invariant in `'a`, and the brand's
    /// `PhantomData<&'a Scope<'a>>` carries that invariance structurally, so it is what pins
    /// `Signature<'a>` invariant in `'a` — no separate marker field is needed.
    decl_scope_ptr: ScopePtr<'a>,
}

impl<'a> Signature<'a> {
    pub fn new(path: String, decl_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            decl_scope_ptr: ScopePtr::erase(decl_scope),
        }
    }

    /// Re-attach `'a` to the stored scope. The branded `decl_scope_ptr` makes this a safe
    /// re-attach: the decl scope is arena-allocated and outlives every `&Signature<'a>` by
    /// construction.
    pub fn decl_scope(&self) -> &'a Scope<'a> {
        self.decl_scope_ptr.reattach()
    }

    /// Stable identity for `KType::Signature { sig, .. }` (its dispatch identity is
    /// `sig.sig_id()` + `pinned_slots`). Each `SIG` declares its own decl_scope and thus a
    /// fresh `ScopeId`; two `SIG Foo = (...)` in the same lexical scope already error
    /// (`Rebind`), so distinct `Signature`s always have distinct ids.
    pub fn sig_id(&self) -> ScopeId {
        self.decl_scope().id
    }
}

#[cfg(test)]
mod tests;
