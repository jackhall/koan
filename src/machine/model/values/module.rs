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
//! The captured scope is held as a plain `&'a Scope<'a>`. A `Module` is bumped at the destination's
//! own `'a` ([`BumpAllocator::value`](crate::witnessed::BumpAllocator)), so the field is already at
//! the region's lifetime with no retype involved; where the whole value rides a lifetime-free
//! carrier, the embedded reference re-anchors with it in that carrier's single audited reattach —
//! exactly as
//! [`KFunction`](crate::machine::core::KFunction) and
//! [`Scope::outer`](crate::machine::core::Scope) hold theirs — so `child_scope` is a bare field
//! read with no per-pointer handle and no `unsafe` of its own.
//!
//! **Built once, then frozen.** A module is assembled complete: construction gathers its members
//! into an owned [`ModuleDraft`] and derives the self-sig from that draft *before* the value
//! exists, so the value itself carries a bumped `path`, two build-once frozen bump-backed tables
//! ([`BumpAllocator::frozen_table`](crate::witnessed::BumpAllocator::frozen_table)) and a plain interned self-sig handle.
//! `Module` is
//! therefore `Copy` and `Drop`-free: it rides the region bump and region death frees it as a chunk
//! ([value-substrates.md § Untyped arenas](../../../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state)).

use std::collections::HashMap;

use crate::machine::core::{RegionBrand, Scope, ScopeId};
use crate::witnessed::BumpBackedMap;

use super::super::types::{
    empty_schema_digest, sig_subtype, KType, Relation, SigSchema, TypeDigest, TypeNode,
    TypeRegistry,
};

/// The owned members a module is assembled from — gathered by a construction site before the value
/// exists, because a built module's maps are frozen. Both maps are keyed by member name and resolve
/// duplicates last-wins, so an overlay (an opaque ascription's per-call mints under its mirrored
/// manifest members) is expressed by insertion order here rather than by a post-alloc write.
///
/// Plain owned data with no lifetime: the draft never enters a region. [`Module::assemble`] re-homes
/// every key at the destination brand on the way in.
#[derive(Default)]
pub struct ModuleDraft {
    /// Member name → type: a mirror of the child scope's type bindings, for a plain `MODULE` and an
    /// opaque view alike (the ascription seeds the view scope with the per-call abstract mints and
    /// the signature's manifest members, then reads them straight back out). A transparent view
    /// reuses its source's child scope and leaves this map empty, reading types through that scope.
    pub type_members: HashMap<String, KType>,
    /// VAL-slot name → the per-call abstract `KType` an opaque ascription minted for the slot's
    /// SIG-declared type. ATTR re-tags a value-side slot read with this identity so
    /// `(int_ord.zero)` reads as the abstract `Type`, not the underlying concrete value. Empty for
    /// unascribed and transparently-ascribed (`:!`) modules.
    pub slot_type_tags: HashMap<String, KType>,
}

impl ModuleDraft {
    /// A draft with no members — a bare module's, and a transparent view's (which reads its members
    /// through the source's child scope rather than a map of its own).
    pub fn empty() -> ModuleDraft {
        ModuleDraft::default()
    }
}

/// First-class module value. `path` is the lexical-source label (`"int_ord"`,
/// `"outer.inner"`). The module value rides the value channel as `KObject::Module(self)` and is
/// typed by its principal signature — the interned `Signature` handle [`Module::ktype`] returns;
/// opaque-ascription members mint `AbstractType { name, nonce: Some(self.scope_id()), .. }`.
#[derive(Clone, Copy)]
pub struct Module<'a> {
    pub path: &'a str,
    child_scope_ref: &'a Scope<'a>,
    /// Member name → type, frozen at assembly from [`ModuleDraft::type_members`]. Lookup is by
    /// content, so a shorter-lived `&str` probe reads it.
    pub type_members: &'a BumpBackedMap<'a, &'a str, KType>,
    /// VAL-slot name → the opaque-ascription tag, frozen at assembly from
    /// [`ModuleDraft::slot_type_tags`]. Empty for unascribed and transparently-ascribed modules.
    pub slot_type_tags: &'a BumpBackedMap<'a, &'a str, KType>,
    /// The module's principal signature (self-sig): the handle naming the interned `Signature`
    /// node this module is typed by. Interned from the draft before the value exists, so "every
    /// mint seals" is structural rather than an invariant a read has to check.
    self_sig: KType,
}

impl<'a> Module<'a> {
    /// **Build a module at its child scope's region and store it there** — the co-located door for a
    /// `Module`, and the reason a stored module never names a region other than the one owning the
    /// scope `MODULE` opened for its body. Its one sibling is the fold brand named below.
    ///
    /// The destination is derived from `child_scope`'s own brand rather than passed alongside it, so
    /// pairing a module with a foreign region is unrepresentable. `path` and the draft's keys may
    /// borrow from anywhere — [`Self::assemble`] re-homes every byte at that same brand before the
    /// value is assembled — and the store is the plain bump verb a `Copy` value takes,
    /// [`BumpAllocator::value`](crate::witnessed::BumpAllocator::value). Nothing is erased and re-anchored on the way in, so no residence audit
    /// stands behind this door: every reference the value holds is a plain `&'a` the borrow checker
    /// already checked against the lifetime the destination brand borrows its region for.
    ///
    /// A module re-tagging a *foreign* child scope has no route here: it is built at a fold brand
    /// instead ([`Scope::store_transparent_view`](crate::machine::core::Scope)), where the borrow it
    /// re-tags is the fold's own operand view.
    pub fn alloc_at_child_scope(
        path: &str,
        child_scope: &'a Scope<'a>,
        draft: ModuleDraft,
        self_sig: KType,
    ) -> &'a Module<'a> {
        let brand = child_scope.brand();
        brand
            .allocator()
            .value(Self::assemble(brand, path, child_scope, draft, self_sig))
    }

    /// Assemble a module value over `child_scope`, re-homing `path` and every draft key into
    /// `brand`'s region and freezing both member tables there
    /// ([`BumpAllocator::frozen_table`](crate::witnessed::BumpAllocator::frozen_table)). Crate-internal and
    /// never a
    /// store: the two doors that place one are [`Self::alloc_at_child_scope`] and the fold brand's
    /// [`FoldingBrand::alloc_module_folded`](crate::machine::core::FoldingBrand), which is how a
    /// transparent-ascribe view re-tags a foreign child scope.
    ///
    /// The single `brand` parameter is the residence discipline: path bytes, key bytes and both
    /// bucket arrays land in one region, and it is the destination's — a `String` key would fail
    /// the table verb's no-drop-glue assert, so the re-home is the only spelling that builds.
    pub(crate) fn assemble<'b>(
        brand: RegionBrand<'b>,
        path: &str,
        child_scope: &'b Scope<'b>,
        draft: ModuleDraft,
        self_sig: KType,
    ) -> Module<'b> {
        let rehome = |entries: HashMap<String, KType>| {
            brand.allocator().frozen_table(
                entries
                    .into_iter()
                    .map(|(name, kt)| (brand.allocator().text(&name) as &'b str, kt)),
            )
        };
        Module {
            path: brand.allocator().text(path),
            child_scope_ref: child_scope,
            type_members: rehome(draft.type_members),
            slot_type_tags: rehome(draft.slot_type_tags),
            self_sig,
        }
    }

    /// The module's type: the handle naming its principal signature. This is what
    /// `KObject::Module(self).ktype()` reports, which is why it takes no registry — the handle was
    /// interned before the value existed.
    pub fn ktype(&self) -> KType {
        self.self_sig
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
