//! `Module` and `ModuleSignature` — first-class module values produced by the `MODULE` and `SIG`
//! builtins. See [design/typing/modules.md](../../../../design/typing/modules.md).
//!
//! **Terminology — "module-signature" vs "expression-signature".** `ModuleSignature` here is the
//! **module-signature** type (`SIG`-declared) — an interface a module can be ascribed to
//! via `:|` / `:!`. The **expression-signature** machinery (`ExpressionSignature`,
//! `Argument`, `SignatureElement`) lives in [`crate::machine::model::types::signature`]. The two
//! are distinct concepts; do not conflate.
//!
//! The captured scope is held as a plain `&'a Scope<'a>` and re-anchored to `'a` together with the
//! rest of the value when the holder is read out of its region (the substrate retype in
//! [`Region::alloc`](crate::witnessed::Region)), exactly as
//! [`KFunction`](crate::machine::core::kfunction::KFunction) and
//! [`Scope::outer`](crate::machine::core::Scope) hold theirs — so `child_scope` / `decl_scope` are
//! bare field reads with no per-pointer handle and no `unsafe` of their own.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

use crate::machine::core::{Scope, ScopeId};

use super::super::types::{sig_subtype, KType, SigSchema};

/// First-class module value. `path` is the lexical-source label (`"IntOrd"`,
/// `"Outer.Inner"`). Opaque-ascription members mint `KType::AbstractType { source:
/// Module(self), name }`; the module value itself rides `KType::Module { module }` — the two
/// are distinguished by `KType` variant.
pub struct Module<'a> {
    pub path: String,
    child_scope_ref: &'a Scope<'a>,
    /// `RefCell` because opaque-ascription installs entries after the surrounding `KObject`
    /// is alloc'd. `Module` is region-pinned and never moved, so a `&'a Module<'a>` borrow
    /// stays valid alongside interior mutation.
    pub type_members: RefCell<HashMap<String, KType<'a>>>,
    /// VAL-slot name → the per-call abstract `KType` an opaque ascription minted for the
    /// slot's SIG-declared type. ATTR re-tags a value-side slot read with this identity so
    /// `(int_ord.zero)` reads as the abstract `Type`, not the underlying concrete value.
    /// Empty for unascribed and transparently-ascribed (`:!`) modules. `RefCell` for the same
    /// reason as `type_members`.
    pub slot_type_tags: RefCell<HashMap<String, KType<'a>>>,
    /// The module's principal signature (self-sig), derived from its body. Sealed exactly once
    /// at the end of construction ([`Module::seal_self_sig`]) and immutable thereafter; a bare
    /// [`Module::new`] with no seal derives it lazily on first read
    /// ([`SigSchema::raw_self_sig`]). The signature-subtyping relation reads it to answer "does
    /// this module satisfy signature `S`". `OnceCell` because — like the maps above — it is
    /// installed after the surrounding value is alloc'd.
    self_sig: OnceCell<SigSchema<'a>>,
    /// Caches `self_sig <: bare-schema(sig)` keyed by `sig.sig_id()`, written by
    /// [`Module::structurally_satisfies`] — the shared entry point both dispatch and ascription
    /// route through. A pure cache — types are immutable, so entries are never invalidated. Pinned
    /// checks are never memoized here (they vary per `KType::Signature` value; see
    /// [`Module::satisfies_pins`]).
    pub satisfaction_memo: RefCell<HashMap<ScopeId, bool>>,
}

impl<'a> Module<'a> {
    pub fn new(path: String, child_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            child_scope_ref: child_scope,
            type_members: RefCell::new(HashMap::new()),
            slot_type_tags: RefCell::new(HashMap::new()),
            self_sig: OnceCell::new(),
            satisfaction_memo: RefCell::new(HashMap::new()),
        }
    }

    /// Install the module's self-sig. Runs exactly once, at the end of construction (after the
    /// `type_members` / `slot_type_tags` writes that feed the derivation) — a double-seal is a
    /// construction bug.
    pub fn seal_self_sig(&self, schema: SigSchema<'a>) {
        if self.self_sig.set(schema).is_err() {
            panic!("self-sig sealed twice on module `{}`", self.path);
        }
    }

    /// The module's self-sig. Returns the sealed schema, or lazily derives it from the body for
    /// a bare [`Module::new`] that was never sealed (e.g. a direct construction in a test).
    pub fn self_sig(&self) -> &SigSchema<'a> {
        self.self_sig.get_or_init(|| SigSchema::raw_self_sig(self))
    }

    /// Pin agreement for a `WITH`-specialized signature slot: every pinned slot names a type
    /// member the self-sig fixes manifest-equal. Self-sigs carry no abstract members, so a
    /// manifest-member lookup is the whole rule — the same manifest agreement `sig_subtype`
    /// applies to a pinned schema's residue.
    pub fn satisfies_pins(&self, pins: &[(String, KType<'a>)]) -> bool {
        let sig = self.self_sig();
        pins.iter()
            .all(|(name, expected)| sig.manifest_members.get(name) == Some(expected))
    }

    /// Structural satisfaction: `self_sig <: bare-schema(sig)` under [`sig_subtype`] — the
    /// admission rule for a signature-typed dispatch slot and the check `:|` / `:!` assert.
    /// Memoized per `sig.sig_id()` in `satisfaction_memo`, both outcomes; a `WITH`-pinned
    /// slot's residue is checked separately via [`Module::satisfies_pins`].
    pub fn structurally_satisfies(&self, sig: &ModuleSignature<'a>) -> bool {
        let sig_id = sig.sig_id();
        let hit = self.satisfaction_memo.borrow().get(&sig_id).copied();
        if let Some(hit) = hit {
            return hit;
        }
        let ok = sig_subtype(self.self_sig(), &SigSchema::of_sig(sig, &[])).is_ok();
        self.satisfaction_memo.borrow_mut().insert(sig_id, ok);
        ok
    }

    pub fn child_scope(&self) -> &'a Scope<'a> {
        self.child_scope_ref
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
    /// `Scope<'a>` is invariant in `'a`, so this reference is what pins `ModuleSignature<'a>`
    /// invariant in `'a` — no separate marker field is needed.
    decl_scope_ref: &'a Scope<'a>,
}

impl<'a> ModuleSignature<'a> {
    pub fn new(path: String, decl_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            decl_scope_ref: decl_scope,
        }
    }

    pub fn decl_scope(&self) -> &'a Scope<'a> {
        self.decl_scope_ref
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
