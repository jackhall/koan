//! `TypeDigest` — the wide content-hash that *is* a `KType`'s identity.
//!
//! Every type carries a digest computed bottom-up from its children (a recursive set at
//! seal, over its finite SCC presentation). Equality is one digest compare and hashing keys
//! on the digest; the width is chosen so an accidental collision is less likely than a
//! hardware fault, so digest equality is type equality with no repair path — the footing
//! [design/typing/type-identity.md](../../../../design/typing/type-identity.md) pins.
//!
//! The digest is a pure function of type content, so two independently built types with the
//! same content digest equal with no shared interner. Generativity is one explicit mechanism
//! applied in two places: a minted `ScopeId` nonce folded into the content ahead of everything
//! else, carried by a recursive-group window (`RecursiveGroupWindow::generative_nonce`) and by an
//! abstract member (`TypeNode::AbstractType`'s `nonce`). Opaque ascription mints the nonce per
//! application, so two
//! `:|` applications of one SIG never unify; a SIG-body declaration carries no nonce and is
//! purely content-keyed. A `Signature` digests by its content's schema (see
//! [`schema_content_digest`]), so two textually identical declarations minted against
//! different scope ids digest identically; the order-independence property is scoped to
//! types without a nonce.
//!
//! **The hasher lives here and only here.** Swapping the hash function or width is a
//! one-file change (the roadmap marks the exact function open, recommending 128-bit
//! truncated BLAKE3, which this ships). Every payload begins with a distinct domain tag
//! byte so no two variants can share a digest, every `String` is length-prefixed so
//! concatenation is unambiguous, and every child digest / `ScopeId` / integer is fed
//! little-endian.

use std::collections::HashMap;

use crate::machine::core::ScopeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::node::{NodeSchema, TypeNode};
use super::record::Record;
use super::registry::TypeRegistry;
use super::sig_schema::SigSchema;
use super::signature::DeferredReturnSurface;

/// A `KType`'s content identity: the low 128 bits of a BLAKE3 hash of its content.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TypeDigest(pub u128);

// Domain tag bytes — one per digestible shape, so no two variants can share a digest even
// with identical trailing payloads. The recursive-group shapes (component, member handle,
// relative sibling) and the two record helpers get their own tags too. These values are
// identity-load-bearing: never reorder or reuse a retired one.
const TAG_NUMBER: u8 = 0x01;
const TAG_STR: u8 = 0x02;
const TAG_BOOL: u8 = 0x03;
const TAG_NULL: u8 = 0x04;
const TAG_IDENTIFIER: u8 = 0x05;
const TAG_KEXPRESSION: u8 = 0x06;
const TAG_SIGILED_TYPE_EXPR: u8 = 0x07;
const TAG_RECORD_TYPE: u8 = 0x08;
const TAG_ANY: u8 = 0x09;
const TAG_OF_KIND: u8 = 0x0A;
const TAG_LIST: u8 = 0x0B;
const TAG_DICT: u8 = 0x0C;
const TAG_RECORD: u8 = 0x0D;
const TAG_KFUNCTION: u8 = 0x0E;
// 0x0F is retired — never reuse it.
const TAG_DEFERRED_RETURN: u8 = 0x10;
const TAG_SET_LOCAL: u8 = 0x11;
// 0x12 (TAG_RECURSIVE_REF) is retired — never reuse it. A not-yet-sealed nominal is named by a
// relative `Sibling` index against its window, which digests under `TAG_SET_LOCAL`.
// 0x13 (TAG_UNRESOLVED) is retired — never reuse it.
const TAG_SET_REF: u8 = 0x14;
const TAG_RECURSIVE_GROUP: u8 = 0x15;
const TAG_UNION: u8 = 0x16;
const TAG_SIGNATURE: u8 = 0x17;
// 0x18 (TAG_MODULE) is retired — never reuse it.
const TAG_ABSTRACT_TYPE: u8 = 0x19;
const TAG_CONSTRUCTOR_APPLY: u8 = 0x1A;
const TAG_RECURSIVE_SET: u8 = 0x1B;
const TAG_SIG_CONTENT: u8 = 0x1C;
const TAG_SIG_SELF_REF: u8 = 0x1D;

/// The one place the hash function is touched. Feeds a domain-tagged, length-prefixed,
/// little-endian byte stream into a BLAKE3 hasher and truncates the result to a `u128`.
struct DigestHasher {
    inner: blake3::Hasher,
}

impl DigestHasher {
    fn new(tag: u8) -> Self {
        let mut inner = blake3::Hasher::new();
        inner.update(&[tag]);
        Self { inner }
    }

    fn byte(&mut self, b: u8) -> &mut Self {
        self.inner.update(&[b]);
        self
    }

    fn count(&mut self, n: usize) -> &mut Self {
        self.inner.update(&(n as u64).to_le_bytes());
        self
    }

    /// A `String`, unambiguously: its byte length as a `u64` LE, then its bytes.
    fn string(&mut self, s: &str) -> &mut Self {
        self.inner.update(&(s.len() as u64).to_le_bytes());
        self.inner.update(s.as_bytes());
        self
    }

    fn digest(&mut self, d: TypeDigest) -> &mut Self {
        self.inner.update(&d.0.to_le_bytes());
        self
    }

    fn scope_id(&mut self, id: ScopeId) -> &mut Self {
        self.inner.update(&id.digest_bytes());
        self
    }

    fn finish(&self) -> TypeDigest {
        let hash = self.inner.finalize();
        let low: [u8; 16] = hash.as_bytes()[..16]
            .try_into()
            .expect("BLAKE3 output is 32 bytes");
        TypeDigest(u128::from_le_bytes(low))
    }
}

/// Stable one-byte tag for a `KKind` — its own discriminant is unstable across enum
/// reordering, so map explicitly.
fn kkind_tag(k: KKind) -> u8 {
    match k {
        KKind::ProperType => 0,
        KKind::Signature => 1,
        KKind::AnyType => 2,
        KKind::NewType => 3,
        KKind::TypeConstructor => 4,
    }
}

/// The digest of a [`TypeNode`] — the identity of the type it interns as, and the key the
/// registry stores it under. The tags and the byte order are identity-load-bearing, and the
/// golden pins hold them fixed.
///
/// Three recipes are not derived from child handles. A [`TypeNode::Sibling`] is its bare index
/// under `TAG_SET_LOCAL`, meaningful only against an ambient window. A [`TypeNode::SetMember`]
/// is `(component digest, index in component)` — its schema is *not* re-fed here, because the
/// component digest was computed over exactly that content at seal. A [`TypeNode::Group`] folds
/// its members' finished handles in declaration order: a group is a declaration boundary that
/// may span several components, so it has no component digest of its own to name.
pub(crate) fn node_digest(node: &TypeNode) -> TypeDigest {
    match node {
        TypeNode::Number => leaf_digest(TAG_NUMBER),
        TypeNode::Str => leaf_digest(TAG_STR),
        TypeNode::Bool => leaf_digest(TAG_BOOL),
        TypeNode::Null => leaf_digest(TAG_NULL),
        TypeNode::Identifier => leaf_digest(TAG_IDENTIFIER),
        TypeNode::KExpression => leaf_digest(TAG_KEXPRESSION),
        TypeNode::SigiledTypeExpr => leaf_digest(TAG_SIGILED_TYPE_EXPR),
        TypeNode::RecordType => leaf_digest(TAG_RECORD_TYPE),
        TypeNode::Any => leaf_digest(TAG_ANY),
        TypeNode::OfKind(k) => of_kind_digest(*k),
        TypeNode::DeferredReturn(surface) => deferred_return_digest(surface),
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            nonce,
        } => abstract_type_digest(*source, name, param_names, *nonce),
        TypeNode::List { element } => list_digest(element.digest()),
        TypeNode::Dict { key, value } => dict_digest(key.digest(), value.digest()),
        TypeNode::Record { fields } => record_digest(fields),
        TypeNode::KFunction { params, ret } => function_digest(params, ret.digest()),
        TypeNode::Union { members } => union_digest(members),
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => constructor_apply_digest(constructor.digest(), arguments),
        TypeNode::Signature { schema_digest, .. } => signature_digest(*schema_digest),
        TypeNode::Sibling(index) => sibling_digest(*index),
        TypeNode::SetMember {
            scc_digest, index, ..
        } => member_ref_digest(*scc_digest, *index),
        TypeNode::Group { members } => {
            let mut h = DigestHasher::new(TAG_RECURSIVE_GROUP);
            h.count(members.len());
            for member in members {
                h.digest(member.digest());
            }
            h.finish()
        }
    }
}

/// A leaf type: its domain tag and nothing else.
fn leaf_digest(tag: u8) -> TypeDigest {
    DigestHasher::new(tag).finish()
}

/// A kind-carrying slot: the tag plus the stable [`kkind_tag`] byte.
fn of_kind_digest(kind: KKind) -> TypeDigest {
    DigestHasher::new(TAG_OF_KIND)
        .byte(kkind_tag(kind))
        .finish()
}

/// A deferred FN return: a discriminant byte for the surface shape, then its rendering.
fn deferred_return_digest(surface: &DeferredReturnSurface) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_DEFERRED_RETURN);
    match surface {
        DeferredReturnSurface::Type(_) => h.byte(0),
        DeferredReturnSurface::Expression(_) => h.byte(1),
    };
    h.string(&surface.render()).finish()
}

/// A relative sibling reference: its bare index under `TAG_SET_LOCAL`, so computing an enclosing
/// component's digest never recurses back into the component.
fn sibling_digest(index: usize) -> TypeDigest {
    DigestHasher::new(TAG_SET_LOCAL).count(index).finish()
}

/// An abstract member's four identity fields: the generativity `nonce` first, then the binder
/// `source`, the name, and the parameter names fed sorted so the encoding is order-blind
/// (identity is the name set, as in [`schema_content_digest`]).
fn abstract_type_digest(
    source: ScopeId,
    name: &str,
    param_names: &[String],
    nonce: Option<ScopeId>,
) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_ABSTRACT_TYPE);
    match nonce {
        Some(id) => {
            h.byte(1).scope_id(id);
        }
        None => {
            h.byte(0);
        }
    }
    h.scope_id(source).string(name).count(param_names.len());
    let mut sorted: Vec<&str> = param_names.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    for param in sorted {
        h.string(param);
    }
    h.finish()
}

// Per-shape digest builders. Each takes its children's handles — which are already their
// digests — so the work is shallow: one hash over one tag and a few `u128`s, never a walk.

/// `List<element>`.
fn list_digest(element: TypeDigest) -> TypeDigest {
    DigestHasher::new(TAG_LIST).digest(element).finish()
}

/// `Dict<key, value>`.
fn dict_digest(key: TypeDigest, value: TypeDigest) -> TypeDigest {
    DigestHasher::new(TAG_DICT)
        .digest(key)
        .digest(value)
        .finish()
}

/// A structural record type.
fn record_digest(fields: &Record<KType>) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_RECORD);
    feed_record(&mut h, fields);
    h.finish()
}

/// A function type `(params) -> ret`.
fn function_digest(params: &Record<KType>, ret: TypeDigest) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_KFUNCTION);
    feed_record(&mut h, params);
    h.digest(ret).finish()
}

/// A union — order-blind, matching its set-based identity: sort the member digests.
fn union_digest(members: &[KType]) -> TypeDigest {
    let mut member_digests: Vec<TypeDigest> = members.iter().map(|m| m.digest()).collect();
    member_digests.sort_unstable();
    let mut h = DigestHasher::new(TAG_UNION);
    h.count(member_digests.len());
    for d in member_digests {
        h.digest(d);
    }
    h.finish()
}

/// `ConstructorApply(ctor, args)` — the args feed name-keyed and name-sorted (see
/// [`feed_record`]), matching the order-blind identity of the args `Record`.
fn constructor_apply_digest(ctor: TypeDigest, args: &Record<KType>) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_CONSTRUCTOR_APPLY);
    h.digest(ctor);
    feed_record(&mut h, args);
    h.finish()
}

/// A module-signature type's digest: its schema's content digest (identity by interface, not by
/// mint — see [type-identity.md](../../../../design/typing/type-identity.md)). `WITH` pins fold
/// into the schema before interning, so the schema content is the whole identity.
fn signature_digest(content_digest: TypeDigest) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIGNATURE);
    h.digest(content_digest);
    h.finish()
}

/// The content digest of a normalized signature schema — a pure function of its members:
/// abstract members `(name, parameter names)`, manifest members `(name, type)`, and value slots
/// `(name, type)`, each group fed in name-sorted order (the maps are unordered). References to
/// the schema's *own* abstract members are canonicalized to a `(TAG_SIG_SELF_REF, name)` leaf, so
/// two textually identical declarations digest identically. Every other minted `AbstractType` (an
/// opaque view's slot tags, a manifest member sourced from another sig) keeps its own digest, so
/// opacity stays generative.
///
/// Reads member content through `types`, so it runs at
/// [`TypeRegistry::signature`](super::registry::TypeRegistry::signature) — once per interned
/// signature — and the resulting digest rides the node.
pub(crate) fn schema_content_digest(schema: &SigSchema, types: &TypeRegistry) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIG_CONTENT);

    // Each abstract member feeds its name, then its order: `0x00` for a first-order proper type,
    // `0x01` + parameter count + the parameter names for a constructor. The names feed sorted, so
    // the encoding is order-blind — parameter identity is the name set.
    let mut abstracts: Vec<(&str, Vec<String>)> = schema
        .abstract_members
        .iter()
        .map(|(name, kt)| {
            let params = match types.node(*kt) {
                TypeNode::AbstractType { param_names, .. } => param_names,
                _ => Vec::new(),
            };
            (name.as_str(), params)
        })
        .collect();
    abstracts.sort_by(|a, b| a.0.cmp(b.0));
    h.count(abstracts.len());
    for (name, param_names) in abstracts {
        h.string(name);
        if param_names.is_empty() {
            h.byte(0);
        } else {
            h.byte(1).count(param_names.len());
            let mut sorted: Vec<&str> = param_names.iter().map(String::as_str).collect();
            sorted.sort_unstable();
            for param in sorted {
                h.string(param);
            }
        }
    }

    feed_named_types(&mut h, &schema.manifest_members, schema, types);
    feed_named_types(&mut h, &schema.value_slots, schema, types);
    h.finish()
}

/// The digest of the member-free schema — the module-lattice top (`:Module`), the type a
/// module-accepting slot lowers to. Byte-for-byte what [`schema_content_digest`] produces for an
/// empty [`SigSchema`] (empty abstract count, then two empty `feed_named_types` headers), and
/// computable without a registry because an empty schema names no member to read. So `:Module`
/// and a user's zero-member `SIG E = ()` share one content identity — an empty interface is an
/// empty interface — and the specificity walk places the top by empty content, not by mint.
pub(crate) fn empty_schema_digest() -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIG_CONTENT);
    h.count(0); // abstract_members
    h.count(0); // manifest_members (feed_named_types header)
    h.count(0); // value_slots (feed_named_types header)
    h.finish()
}

/// Feed a `name -> type` member map into `h` in name-sorted order (the map is unordered), each
/// type digested through [`canonical_type_digest`] so self-member references collapse to a name
/// leaf. Shared by manifest members and value slots.
fn feed_named_types(
    h: &mut DigestHasher,
    members: &HashMap<String, KType>,
    schema: &SigSchema,
    types: &TypeRegistry,
) {
    let mut pairs: Vec<(&str, TypeDigest)> = members
        .iter()
        .map(|(name, kt)| (name.as_str(), canonical_type_digest(*kt, schema, types)))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    h.count(pairs.len());
    for (name, d) in pairs {
        h.string(name).digest(d);
    }
}

/// A member type's digest, with references to `schema`'s own abstract members canonicalized to a
/// `(TAG_SIG_SELF_REF, name)` leaf. Recurses through exactly the composite shapes
/// [`substitute_sig_members`](super::sig_schema::substitute_sig_members) rewrites — List, Dict,
/// Record, KFunction, Union, ConstructorApply — rebuilding each composite's digest from its
/// canonicalized children. Every other shape contributes its handle's own digest, so a minted
/// `AbstractType` from another source keeps its generative identity.
fn canonical_type_digest(kt: KType, schema: &SigSchema, types: &TypeRegistry) -> TypeDigest {
    let node = types.node(kt);
    if let Some(name) = schema_self_ref(&node, schema) {
        return DigestHasher::new(TAG_SIG_SELF_REF).string(name).finish();
    }
    match node {
        TypeNode::List { element } => DigestHasher::new(TAG_LIST)
            .digest(canonical_type_digest(element, schema, types))
            .finish(),
        TypeNode::Dict { key, value } => DigestHasher::new(TAG_DICT)
            .digest(canonical_type_digest(key, schema, types))
            .digest(canonical_type_digest(value, schema, types))
            .finish(),
        TypeNode::Record { fields } => {
            let mut h = DigestHasher::new(TAG_RECORD);
            feed_record_canonical(&mut h, &fields, schema, types);
            h.finish()
        }
        TypeNode::KFunction { params, ret } => {
            let mut h = DigestHasher::new(TAG_KFUNCTION);
            feed_record_canonical(&mut h, &params, schema, types);
            h.digest(canonical_type_digest(ret, schema, types)).finish()
        }
        TypeNode::Union { members } => {
            let mut member_digests: Vec<TypeDigest> = members
                .iter()
                .map(|m| canonical_type_digest(*m, schema, types))
                .collect();
            member_digests.sort_unstable();
            let mut h = DigestHasher::new(TAG_UNION);
            h.count(member_digests.len());
            for d in member_digests {
                h.digest(d);
            }
            h.finish()
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            let mut h = DigestHasher::new(TAG_CONSTRUCTOR_APPLY);
            h.digest(canonical_type_digest(constructor, schema, types));
            feed_record_canonical(&mut h, &arguments, schema, types);
            h.finish()
        }
        _ => kt.digest(),
    }
}

/// `Some(member name)` iff `node` is a reference to one of `schema`'s own abstract members — a
/// nonce-free `AbstractType` sourced at the schema's binder, of either order (a first-order slot
/// type, or the constructor position of a `ConstructorApply` / a bare higher-kinded slot). The
/// shape `substitute_sig_members` rewrites. `None` for a self-sig (no binder, no abstract
/// members), so a self-sig's member digests are its content unchanged — including any generative
/// `AbstractType` mints, which carry a nonce and stay id-keyed.
fn schema_self_ref<'n>(node: &'n TypeNode, schema: &SigSchema) -> Option<&'n str> {
    match node {
        TypeNode::AbstractType {
            source,
            name,
            nonce: None,
            ..
        } if schema.sig_id == Some(*source) => Some(name),
        _ => None,
    }
}

/// [`feed_record`] with each field type routed through [`canonical_type_digest`] — the canonical
/// twin used inside a signature's schema-content walk.
fn feed_record_canonical(
    h: &mut DigestHasher,
    record: &Record<KType>,
    schema: &SigSchema,
    types: &TypeRegistry,
) {
    let mut pairs: Vec<(&str, TypeDigest)> = record
        .iter()
        .map(|(name, value)| (name.as_str(), canonical_type_digest(*value, schema, types)))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    h.count(pairs.len());
    for (name, d) in pairs {
        h.string(name).digest(d);
    }
}

/// A sealed member's identity: its strongly-connected component's digest plus its index in that
/// component's canonical (member-name) order. The single derivation of a member handle — the seal
/// mints one per member, and every later consumer that knows a component recomputes the same value
/// rather than storing a sibling list.
///
/// The `byte(1)` is a fixed prefix of the recipe, not a discriminant: nothing pre-seal is
/// digestible, so there is no second arm to distinguish.
pub(crate) fn member_ref_digest(scc_digest: TypeDigest, index: usize) -> TypeDigest {
    DigestHasher::new(TAG_SET_REF)
        .byte(1)
        .digest(scc_digest)
        .count(index)
        .finish()
}

/// Order-blind record digest: `(name, field digest)` pairs sorted by name, each
/// length-prefixed. Matches `Record`'s `IndexMap` (order-blind) equality. Shared by
/// `Record` and `KFunction` params.
fn feed_record(h: &mut DigestHasher, record: &Record<KType>) {
    let mut pairs: Vec<(&str, TypeDigest)> = record
        .iter()
        .map(|(name, value)| (name.as_str(), value.digest()))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    h.count(pairs.len());
    for (name, d) in pairs {
        h.string(name).digest(d);
    }
}

/// One member as the component recipe presents it: its name, its kind, and its schema with every
/// sibling handle already re-encoded — intra-component references as a relative
/// [`TypeNode::Sibling`] into the component's own canonical order, cross-component references as
/// the referent's finished member handle.
pub(crate) struct ComponentMember<'m> {
    pub name: &'m str,
    pub kind: KKind,
    pub schema: &'m NodeSchema,
}

/// The content digest of one strongly-connected component of a recursive-group window — the
/// identity half of every member handle it contains (see [`member_ref_digest`]).
///
/// `members` arrive in the component's canonical **name** order, so two independently declared
/// components with the same content present identically whatever order they were written in.
/// A generative component (opaque ascription's per-application mint) folds its nonce first, so two
/// applications never unify; every other component is content-only. Intra-component sibling
/// references digest as bare relative indices, so computing a component's digest never recurses
/// back into the component.
///
/// A singleton component is byte-identical to the whole-declaration recipe it generalizes: count
/// `1`, one member, its own self-reference relative index `0`. That is what keeps every standalone
/// `NEWTYPE` / `UNION` / opaque mint at the digest it had before identity became per-component.
pub(crate) fn component_digest(
    generative_nonce: Option<ScopeId>,
    members: &[ComponentMember<'_>],
) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_RECURSIVE_SET);
    match generative_nonce {
        Some(nonce) => {
            h.byte(1).scope_id(nonce);
        }
        None => {
            h.byte(0);
        }
    }
    h.count(members.len());
    for member in members {
        h.string(member.name).byte(kkind_tag(member.kind));
        match member.schema {
            NodeSchema::NewType(repr) => {
                h.byte(0).digest(repr.digest());
            }
            NodeSchema::TypeConstructor {
                schema,
                param_names,
            } => {
                // HashMap iteration order is nondeterministic — sort by key.
                let mut entries: Vec<(&str, TypeDigest)> = schema
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.digest()))
                    .collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                h.byte(1).count(entries.len());
                for (name, d) in entries {
                    h.string(name).digest(d);
                }
                h.count(param_names.len());
                for p in param_names {
                    h.string(p);
                }
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests;
