//! `Module` — the first-class module value produced by the `MODULE` builtin. See
//! [design/typing/modules.md](../../../../design/typing/modules.md).
//!
//! **Terminology — "module-signature" vs "expression-signature".** A module-signature is the
//! interface a module can be ascribed to via `:|` / `:!` — a `SIG`-declared interface or a
//! module's own self-sig, both carried as an owned
//! [`SigContent`](crate::machine::model::types::SigContent) wrapping a
//! [`SigSchema`](crate::machine::model::types::SigSchema) (see [`Module::self_sig_content`]). The
//! **expression-signature** machinery (`ExpressionSignature`, `Argument`, `SignatureElement`)
//! lives in [`crate::machine::model::types::signature`]. The two are distinct concepts; do not
//! conflate.
//!
//! The captured scope is held as a plain `&'a Scope<'a>` and re-anchored to `'a` together with the
//! rest of the value when the holder is read out of its region (the substrate retype in
//! [`Region::alloc`](crate::witnessed::Region)), exactly as
//! [`KFunction`](crate::machine::core::KFunction) and
//! [`Scope::outer`](crate::machine::core::Scope) hold theirs — so `child_scope` is a bare field
//! read with no per-pointer handle and no `unsafe` of its own.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::{Scope, ScopeId};

use super::super::types::{
    sig_subtype, KType, Relation, SigContent, SigSchema, TypeDigest, TypeRegistry,
};

/// First-class module value. `path` is the lexical-source label (`"int_ord"`,
/// `"outer.inner"`). The module value rides the value channel as `KObject::Module(self)` and is
/// typed by its principal signature (`KType::Signature { content: self.self_sig_content(), .. }`);
/// opaque-ascription members mint `KType::AbstractType { source: self.scope_id(), name }`.
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
    /// The module's principal signature (self-sig), owned and wrapped in an `Rc` — the same
    /// bundle a `KType::Signature { content, .. }` over this module shares by `Rc::clone`.
    /// Sealed exactly once at the end of construction ([`Module::seal_self_sig`]) and immutable
    /// thereafter; a bare [`Module::new`] with no seal derives it lazily on first read
    /// ([`SigSchema::raw_self_sig`]). The signature-subtyping relation reads it to answer "does
    /// this module satisfy signature `S`". `OnceCell` because — like the maps above — it is
    /// installed after the surrounding value is alloc'd.
    self_sig: OnceCell<Rc<SigContent<'a>>>,
}

impl<'a> Module<'a> {
    pub fn new(path: String, child_scope: &'a Scope<'a>) -> Self {
        Self {
            path,
            child_scope_ref: child_scope,
            type_members: RefCell::new(HashMap::new()),
            slot_type_tags: RefCell::new(HashMap::new()),
            self_sig: OnceCell::new(),
        }
    }

    /// Install the module's self-sig. Runs exactly once, at the end of construction (after the
    /// `type_members` / `slot_type_tags` writes that feed the derivation) — a double-seal is a
    /// construction bug. Wraps `schema` into an `Rc<SigContent>`, computing its digest.
    pub fn seal_self_sig(&self, schema: SigSchema<'a>) {
        let content = Rc::new(SigContent::new(self.path.clone(), self.scope_id(), schema));
        if self.self_sig.set(content).is_err() {
            panic!("self-sig sealed twice on module `{}`", self.path);
        }
    }

    /// The module's self-sig content — the identity a `Signature { content: self.self_sig_content(),
    /// .. }` carries. Returns the sealed content, or lazily derives + wraps it from the body for a
    /// bare [`Module::new`] that was never sealed (e.g. a direct construction in a test).
    pub fn self_sig_content(&self) -> &Rc<SigContent<'a>> {
        self.self_sig.get_or_init(|| {
            let schema = SigSchema::raw_self_sig(self);
            Rc::new(SigContent::new(self.path.clone(), self.scope_id(), schema))
        })
    }

    /// The module's self-sig schema (see [`Self::self_sig_content`]).
    pub fn self_sig(&self) -> &SigSchema<'a> {
        &self.self_sig_content().schema
    }

    /// The module's self-sig content digest — the `SigSatisfies` verdict subject key
    /// (`registry.rs`). Reads the cached digest on [`Self::self_sig_content`].
    pub fn self_sig_digest(&self) -> TypeDigest {
        self.self_sig_content().schema_digest
    }

    /// Pin agreement for a `WITH`-specialized signature slot: every pinned slot names a type
    /// member the self-sig fixes manifest-equal. Self-sigs carry no abstract members, so a
    /// manifest-member lookup is the whole rule — the same manifest agreement `sig_subtype`
    /// applies to a pinned schema's residue.
    pub fn satisfies_pins<'p>(&self, pins: &[(String, KType<'p>)]) -> bool {
        let sig = self.self_sig();
        pins.iter().all(|(name, expected)| {
            sig.manifest_members
                .get(name)
                .is_some_and(|m| m == expected)
        })
    }

    /// Whether this module satisfies the signature content `c` — the admission rule a
    /// `KType::Signature` slot applies to a module value (pins are checked separately by the
    /// caller, as they live on `KType::Signature`, not here; see [`Self::satisfies_pins`]). The
    /// single entry point for module satisfaction: `c.is_empty_interface()` admits every module
    /// (the lattice top); a digest-equal `c` short-circuits (sound by reflexivity of
    /// `sig_subtype`, and broader than a same-module check — any content-equal pair matches, not
    /// just the same module); otherwise consults the run's type registry under `SigSatisfies`,
    /// keyed by this module's and `c`'s raw schema digests, both outcomes recorded.
    pub fn satisfies_sig_content(&self, c: &SigContent, types: &TypeRegistry) -> bool {
        if c.is_empty_interface() {
            return true;
        }
        let subject = self.self_sig_digest();
        if subject == c.schema_digest {
            return true;
        }
        if let Some(hit) = types.verdict(subject, c.schema_digest, Relation::SigSatisfies) {
            return hit;
        }
        let ok = sig_subtype(self.self_sig(), &c.schema, types).is_ok();
        types.record_verdict(subject, c.schema_digest, Relation::SigSatisfies, ok);
        ok
    }

    pub fn child_scope(&self) -> &'a Scope<'a> {
        self.child_scope_ref
    }

    /// Stable identity: the key every module-keyed `KType` compares and digests on (an
    /// `AbstractType` minted from this module). Two distinct opaque ascriptions of the same source
    /// module compare distinct because each allocates a fresh child scope (and thus a fresh
    /// `ScopeId`).
    pub fn scope_id(&self) -> ScopeId {
        self.child_scope().id
    }
}

#[cfg(test)]
mod tests;
