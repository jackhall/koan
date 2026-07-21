//! `Module` — the first-class module value produced by the `MODULE` builtin. See
//! [design/typing/modules.md](../../../../design/typing/modules.md).
//!
//! **Terminology — "module-signature" vs "expression-signature".** A module-signature is the
//! interface a module can be ascribed to via `:|` / `:!` — a `SIG`-declared interface or a
//! module's own self-sig, both interned as a `Signature` node over a
//! [`SigSchema`](crate::machine::model::types::SigSchema) and named by one
//! [`KType`] handle (see [`Module::ktype`]). The **expression-signature** machinery
//! (`ExpressionSignature`, `Argument`, `SignatureElement`) lives in
//! [`crate::machine::model::types::signature`]. The two are distinct concepts; do not conflate.
//!
//! The captured scope is held as a plain `&'a Scope<'a>` and re-anchored to `'a` together with the
//! rest of the value when the holder is read out of its region (the substrate retype in
//! [`Region::alloc`](crate::witnessed::Region)), exactly as
//! [`KFunction`](crate::machine::core::KFunction) and
//! [`Scope::outer`](crate::machine::core::Scope) hold theirs — so `child_scope` is a bare field
//! read with no per-pointer handle and no `unsafe` of its own.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

use crate::machine::core::{Scope, ScopeId};

use super::super::types::{
    empty_schema_digest, sig_subtype, KType, Relation, SigSchema, TypeDigest, TypeNode,
    TypeRegistry,
};

/// First-class module value. `path` is the lexical-source label (`"int_ord"`,
/// `"outer.inner"`). The module value rides the value channel as `KObject::Module(self)` and is
/// typed by its principal signature — the interned `Signature` handle [`Module::ktype`] returns;
/// opaque-ascription members mint `AbstractType { name, nonce: Some(self.scope_id()), .. }`.
pub struct Module<'a> {
    pub path: String,
    child_scope_ref: &'a Scope<'a>,
    /// `RefCell` because opaque-ascription installs entries after the surrounding `KObject`
    /// is alloc'd. `Module` is region-pinned and never moved, so a `&'a Module<'a>` borrow
    /// stays valid alongside interior mutation.
    pub type_members: RefCell<HashMap<String, KType>>,
    /// VAL-slot name → the per-call abstract `KType` an opaque ascription minted for the
    /// slot's SIG-declared type. ATTR re-tags a value-side slot read with this identity so
    /// `(int_ord.zero)` reads as the abstract `Type`, not the underlying concrete value.
    /// Empty for unascribed and transparently-ascribed (`:!`) modules. `RefCell` for the same
    /// reason as `type_members`.
    pub slot_type_tags: RefCell<HashMap<String, KType>>,
    /// The module's principal signature (self-sig): the handle naming the interned `Signature`
    /// node this module is typed by. Sealed exactly once at the end of construction
    /// ([`Module::seal_self_sig`]) and immutable thereafter. `OnceCell` because — like the maps
    /// above — it is installed after the surrounding value is alloc'd, and because the derivation
    /// reads `type_members` / `slot_type_tags`, which construction writes first.
    ///
    /// Every mint seals: an unfilled cell is a construction bug, not a state, so the reads below
    /// panic rather than deriving anything late.
    self_sig: OnceCell<KType>,
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
    /// construction bug. Interns `schema` as a `Signature` node and stores its handle.
    pub fn seal_self_sig(&self, schema: SigSchema, types: &TypeRegistry) {
        let handle = types.signature(schema);
        if self.self_sig.set(handle).is_err() {
            panic!("self-sig sealed twice on module `{}`", self.path);
        }
    }

    /// The module's type: the handle naming its principal signature, copied out of the sealed
    /// cell. This is what `KObject::Module(self).ktype()` reports, which is why it takes no
    /// registry — every mint seals, so the handle is already interned.
    pub fn ktype(&self) -> KType {
        *self.self_sig.get().unwrap_or_else(|| {
            panic!(
                "module `{}` was surfaced before its self-sig was sealed",
                self.path
            )
        })
    }

    /// The module's self-sig schema, cloned out of its signature node.
    pub fn self_sig(&self, types: &TypeRegistry) -> SigSchema {
        match types.node(self.ktype()) {
            TypeNode::Signature { schema, .. } => schema,
            _ => panic!("module `{}`'s self-sig is not a signature node", self.path),
        }
    }

    /// The module's self-sig content digest — the `SigSatisfies` verdict subject key
    /// (`registry.rs`). Reads the digest the signature node computed once at intern time.
    pub fn self_sig_digest(&self, types: &TypeRegistry) -> TypeDigest {
        match types.node(self.ktype()) {
            TypeNode::Signature { schema_digest, .. } => schema_digest,
            _ => panic!("module `{}`'s self-sig is not a signature node", self.path),
        }
    }

    /// Whether this module satisfies the interface `schema` — the admission rule a signature
    /// slot applies to a module value (a `WITH` pin is a manifest member of the folded schema,
    /// checked by the same relation). The single entry point for
    /// module satisfaction: the empty interface admits every module (the lattice top); a
    /// digest-equal schema short-circuits (sound by reflexivity of `sig_subtype`, and broader
    /// than a same-module check — any content-equal pair matches, not just the same module);
    /// otherwise consults the run's type registry under `SigSatisfies`, keyed by this module's
    /// and `schema`'s digests, both outcomes recorded.
    pub fn satisfies_sig_schema(
        &self,
        schema: &SigSchema,
        schema_digest: TypeDigest,
        types: &TypeRegistry,
    ) -> bool {
        if schema_digest == empty_schema_digest() {
            return true;
        }
        let subject = self.self_sig_digest(types);
        if subject == schema_digest {
            return true;
        }
        if let Some(hit) = types.verdict(subject, schema_digest, Relation::SigSatisfies) {
            return hit;
        }
        let ok = sig_subtype(&self.self_sig(types), schema, types).is_ok();
        types.record_verdict(subject, schema_digest, Relation::SigSatisfies, ok);
        ok
    }

    pub fn child_scope(&self) -> &'a Scope<'a> {
        self.child_scope_ref
    }

    /// Stable identity: the generativity nonce every opaque-ascription mint out of this module
    /// carries (an `AbstractType`'s `nonce`, a generative set's `generative_nonce`). Two distinct
    /// opaque ascriptions of the same source module compare distinct because each allocates a
    /// fresh child scope (and thus a fresh `ScopeId`).
    pub fn scope_id(&self) -> ScopeId {
        self.child_scope().id
    }
}

#[cfg(test)]
mod tests;
