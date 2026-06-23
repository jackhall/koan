//! `Module` and `ModuleSignature` — first-class module values produced by the `MODULE` and `SIG`
//! builtins. See [design/typing/modules.md](../../../../design/typing/modules.md).
//!
//! **Terminology — "module-signature" vs "expression-signature".** `ModuleSignature` here is the
//! **module-signature** type (`SIG`-declared) — an interface a module can be ascribed to
//! via `:|` / `:!`. The **expression-signature** machinery (`ExpressionSignature`,
//! `Argument`, `SignatureElement`) lives in [`crate::machine::model::types::signature`]. The two
//! are distinct concepts; do not conflate.
//!
//! Lifetime erasure on the scope pointer routes through
//! [`BoundedScopePtr`](crate::machine::core::scope_ptr::BoundedScopePtr), the same content-branded
//! handle [`KFunction`](crate::machine::core::kfunction::KFunction) and
//! [`Scope::outer`](crate::machine::core::Scope) use. `Module` / `ModuleSignature` own a real `'a`,
//! so the brand makes `child_scope` / `decl_scope` **safe** reader-bounded re-hands; the irreducible
//! `unsafe` re-attach lives at the lifetime-free carriers
//! ([`ErasedScopePtr`](crate::machine::core::ErasedScopePtr)).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::machine::core::{BoundedScopePtr, Scope, ScopeId};

use super::super::types::KType;

/// First-class module value. `path` is the lexical-source label (`"IntOrd"`,
/// `"Outer.Inner"`); `type_members` maps the module's abstract type names to the `KType`
/// they currently expose. Opaque-ascription members mint `KType::AbstractType { source:
/// Module(self), name }`; the module value itself rides `KType::Module { module, frame }`
/// in the surrounding `Carried::Type` (the two are distinguished by `KType` variant —
/// `AbstractType` vs `Module`).
pub struct Module<'a> {
    pub path: String,
    child_scope_ptr: BoundedScopePtr<'a>,
    /// `RefCell` because opaque-ascription installs entries after the surrounding `KObject`
    /// is alloc'd. `Module` is region-pinned and never moved, so a `&'a Module<'a>` borrow
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
    /// writes after the surrounding `Module` value is already alloc'd.
    pub compatible_sigs: RefCell<Vec<ScopeId>>,
}

impl<'a> Module<'a> {
    pub fn new(path: String, child_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            child_scope_ptr: BoundedScopePtr::erase(child_scope),
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

    /// Re-hand the captured child scope with the borrow bounded by the `&self` receiver and the
    /// content `'a` left free. The branded `child_scope_ptr` makes this a **safe** re-hand: it
    /// consumed a real `&Scope<'a>` at construction, and the region outlives every `&Module<'a>` by
    /// construction.
    pub fn child_scope(&self) -> &Scope<'a> {
        self.child_scope_ptr.get()
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
pub struct ModuleSignature<'a> {
    pub path: String,
    /// Branded [`BoundedScopePtr<'a>`]: `Scope<'a>` is invariant in `'a`, and the brand's
    /// `PhantomData<&'a Scope<'a>>` carries that invariance structurally, so it is what pins
    /// `ModuleSignature<'a>` invariant in `'a` — no separate marker field is needed.
    decl_scope_ptr: BoundedScopePtr<'a>,
}

impl<'a> ModuleSignature<'a> {
    pub fn new(path: String, decl_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            decl_scope_ptr: BoundedScopePtr::erase(decl_scope),
        }
    }

    /// Re-hand the decl scope with the borrow bounded by the `&self` receiver and content `'a`
    /// left free. The branded `decl_scope_ptr` makes this a **safe** re-hand: the decl scope is
    /// region-allocated and outlives every `&ModuleSignature<'a>` by construction.
    pub fn decl_scope(&self) -> &Scope<'a> {
        self.decl_scope_ptr.get()
    }

    /// Stable identity for `KType::Signature { sig, .. }` (its dispatch identity is
    /// `sig.sig_id()` + `pinned_slots`). Each `SIG` declares its own decl_scope and thus a
    /// fresh `ScopeId`; two `SIG Foo = (...)` in the same lexical scope already error
    /// (`Rebind`), so distinct `ModuleSignature`s always have distinct ids.
    pub fn sig_id(&self) -> ScopeId {
        self.decl_scope().id
    }
}

#[cfg(test)]
mod tests;
