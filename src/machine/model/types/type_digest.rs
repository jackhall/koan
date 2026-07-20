//! `TypeDigest` — the wide content-hash that *is* a `KType`'s identity.
//!
//! Every type carries a digest computed bottom-up from its children (a recursive set at
//! seal, over its finite SCC presentation). Equality is one digest compare and hashing keys
//! on the digest; the width is chosen so an accidental collision is less likely than a
//! hardware fault, so digest equality is type equality with no repair path — the footing
//! [design/typing/type-identity.md](../../../../design/typing/type-identity.md) pins.
//!
//! The digest is a pure function of type content, so two independently built types with the
//! same content digest equal with no shared interner. The two generative exceptions fold a
//! minted `ScopeId` into the content — opaque ascription's per-application nonce (a set's
//! `generative_nonce`) and the sole id-keyed leaf, `AbstractType` — so distinct abstractions
//! stay distinct. A `Signature` digests by its content's schema (see
//! [`schema_content_digest`]), so two textually identical declarations minted against
//! different scope ids digest identically; the order-independence property is scoped to
//! types without a minted leaf.
//!
//! **The hasher lives here and only here.** Swapping the hash function or width is a
//! one-file change (the roadmap marks the exact function open, recommending 128-bit
//! truncated BLAKE3, which this ships). Every payload begins with a distinct domain tag
//! byte so no two variants can share a digest, every `String` is length-prefixed so
//! concatenation is unambiguous, and every child digest / `ScopeId` / integer is fed
//! little-endian.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::ScopeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::record::Record;
use super::recursive_set::{NominalSchema, RecursiveSet};
use super::sig_schema::SigSchema;
use super::signature::DeferredReturnSurface;

/// A `KType`'s content identity: the low 128 bits of a BLAKE3 hash of its content.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TypeDigest(pub u128);

// Domain tag bytes — one per digestible shape, so no two variants can share a digest even
// with identical trailing payloads. `RecursiveSet` and the two record helpers get their own
// tags too. These values are identity-load-bearing: never reorder or reuse a retired one.
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
const TAG_RECURSIVE_REF: u8 = 0x12;
const TAG_UNRESOLVED: u8 = 0x13;
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

/// The digest of a `KType` — its identity. The composite variants store their digest
/// (filled by the smart constructors on [`KType`]), so this reads the field; the leaf,
/// id-keyed, and member-reference variants compute theirs on demand. For a member reference
/// (`SetRef` / `RecursiveGroup`) that means the set's sealed digest; a pre-seal reference (no
/// digest yet) contributes a pointer-derived transient that never survives sealing — the
/// whole composite is rebuilt at seal (`seal_recursive_refs` re-runs the constructors),
/// recomputing a content digest.
pub fn digest_of(kt: &KType) -> TypeDigest {
    match kt {
        // Composite variants store their digest — the smart constructors filled it.
        KType::List { digest, .. }
        | KType::Dict { digest, .. }
        | KType::Record { digest, .. }
        | KType::KFunction { digest, .. }
        | KType::Union { digest, .. }
        | KType::Signature { digest, .. }
        | KType::ConstructorApply { digest, .. } => *digest,
        KType::Number => DigestHasher::new(TAG_NUMBER).finish(),
        KType::Str => DigestHasher::new(TAG_STR).finish(),
        KType::Bool => DigestHasher::new(TAG_BOOL).finish(),
        KType::Null => DigestHasher::new(TAG_NULL).finish(),
        KType::Identifier => DigestHasher::new(TAG_IDENTIFIER).finish(),
        KType::KExpression => DigestHasher::new(TAG_KEXPRESSION).finish(),
        KType::SigiledTypeExpr => DigestHasher::new(TAG_SIGILED_TYPE_EXPR).finish(),
        KType::RecordType => DigestHasher::new(TAG_RECORD_TYPE).finish(),
        KType::Any => DigestHasher::new(TAG_ANY).finish(),
        KType::OfKind(k) => DigestHasher::new(TAG_OF_KIND).byte(kkind_tag(*k)).finish(),
        KType::DeferredReturn(surface) => {
            let mut h = DigestHasher::new(TAG_DEFERRED_RETURN);
            match surface {
                DeferredReturnSurface::Type(_) => h.byte(0),
                DeferredReturnSurface::Expression(_) => h.byte(1),
            };
            h.string(&surface.render()).finish()
        }
        KType::SetLocal(index) => DigestHasher::new(TAG_SET_LOCAL).count(*index).finish(),
        KType::RecursiveRef(name) => DigestHasher::new(TAG_RECURSIVE_REF).string(name).finish(),
        KType::Unresolved(ti) => DigestHasher::new(TAG_UNRESOLVED)
            .string(&ti.render())
            .finish(),
        KType::SetRef { set, index } => set_ref_digest(set, *index),
        KType::RecursiveGroup(set) => {
            let mut h = DigestHasher::new(TAG_RECURSIVE_GROUP);
            feed_set_identity(&mut h, set);
            h.finish()
        }
        // `param_names` is excluded, matching `PartialEq`: one source-and-name binds one member.
        KType::AbstractType { source, name, .. } => DigestHasher::new(TAG_ABSTRACT_TYPE)
            .scope_id(*source)
            .string(name)
            .finish(),
    }
}

// Per-shape digest builders — the smart constructors on `KType` call these to fill a
// composite's `digest` field from its children's stored digests (shallow work). Each mirrors
// exactly the fields the corresponding `PartialEq` arm compares, keeping `a == b ⟹
// digest(a) == digest(b)`.

/// `List<element>`.
pub(crate) fn list_digest(element: TypeDigest) -> TypeDigest {
    DigestHasher::new(TAG_LIST).digest(element).finish()
}

/// `Dict<key, value>`.
pub(crate) fn dict_digest(key: TypeDigest, value: TypeDigest) -> TypeDigest {
    DigestHasher::new(TAG_DICT)
        .digest(key)
        .digest(value)
        .finish()
}

/// A structural record type.
pub(crate) fn record_digest(fields: &Record<KType>) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_RECORD);
    feed_record(&mut h, fields);
    h.finish()
}

/// A function type `(params) -> ret`.
pub(crate) fn function_digest(params: &Record<KType>, ret: TypeDigest) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_KFUNCTION);
    feed_record(&mut h, params);
    h.digest(ret).finish()
}

/// A union — order-blind, matching the set-based `PartialEq`: sort the member digests.
pub(crate) fn union_digest(members: &[KType]) -> TypeDigest {
    let mut member_digests: Vec<TypeDigest> = members.iter().map(KType::digest).collect();
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
pub(crate) fn constructor_apply_digest(ctor: TypeDigest, args: &Record<KType>) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_CONSTRUCTOR_APPLY);
    h.digest(ctor);
    feed_record(&mut h, args);
    h.finish()
}

/// A module-signature type's digest: its content's schema digest (identity by interface, not by
/// mint — see [type-identity.md](../../../../design/typing/type-identity.md)) wrapped with the
/// `WITH` pins that specialize it. Positional over `pinned_slots` (matching `PartialEq`; do NOT
/// sort).
pub(crate) fn signature_digest(
    content_digest: TypeDigest,
    pinned_slots: &[(String, KType)],
) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIGNATURE);
    h.digest(content_digest);
    h.count(pinned_slots.len());
    for (name, kt) in pinned_slots {
        h.string(name).digest(kt.digest());
    }
    h.finish()
}

/// The content digest of a normalized signature schema — a pure function of its members:
/// abstract members `(name, parameter names)`, manifest members `(name, type)`, and value slots
/// `(name, type)`, each group fed in name-sorted order (the maps are unordered). References to
/// the schema's *own* abstract members are canonicalized to a `(TAG_SIG_SELF_REF, name)` leaf, so
/// two textually identical declarations — whose members are minted against different scope ids —
/// digest identically. Every other minted `AbstractType` (an opaque view's slot tags, a manifest
/// member sourced from another sig) keeps its id-keyed stored digest, so opacity stays generative.
/// A `SigContent` caches this once at construction (see `Module::self_sig_digest`,
/// `SigContent::schema_digest`).
pub(crate) fn schema_content_digest(schema: &SigSchema) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIG_CONTENT);

    // Each abstract member feeds its name, then its order: `0x00` for a first-order proper type,
    // `0x01` + parameter count + the parameter names for a constructor. The names feed sorted, so
    // the encoding is order-blind — parameter identity is the name set.
    let mut abstracts: Vec<(&str, &[String])> = schema
        .abstract_members
        .iter()
        .map(|(name, kt)| {
            let params: &[String] = match kt {
                KType::AbstractType { param_names, .. } => param_names,
                _ => &[],
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

    feed_named_types(&mut h, &schema.manifest_members, schema);
    feed_named_types(&mut h, &schema.value_slots, schema);
    h.finish()
}

/// The digest [`SigContent::empty`](super::sig_schema::SigContent::empty) folds in — the
/// module-lattice top (`:Module`), the type a module-accepting slot lowers to. It is the content
/// digest of a zero-member schema, byte-for-byte what [`schema_content_digest`] produces for an
/// empty [`SigSchema`] (empty abstract count, then two empty `feed_named_types` headers). So
/// `:Module` and a user's zero-member `SIG E = ()` share one content identity — an empty
/// interface is an empty interface — and the specificity walk places the top by empty content,
/// not by mint.
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
fn feed_named_types(h: &mut DigestHasher, members: &HashMap<String, KType>, schema: &SigSchema) {
    let mut pairs: Vec<(&str, TypeDigest)> = members
        .iter()
        .map(|(name, kt)| (name.as_str(), canonical_type_digest(kt, schema)))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    h.count(pairs.len());
    for (name, d) in pairs {
        h.string(name).digest(d);
    }
}

/// A member type's digest, with references to `schema`'s own abstract members canonicalized to a
/// `(TAG_SIG_SELF_REF, name)` leaf. Recurses through exactly the composite variants
/// [`substitute_sig_members`](super::sig_schema::substitute_sig_members) rewrites — List, Dict,
/// Record, KFunction, Union, ConstructorApply — rebuilding each composite's digest from its
/// canonicalized children (mirroring the per-shape builders in this file). Every other variant
/// contributes its stored digest, so a minted `AbstractType` from another source keeps its
/// generative identity.
fn canonical_type_digest(kt: &KType, schema: &SigSchema) -> TypeDigest {
    if let Some(name) = schema_self_ref(kt, schema) {
        return DigestHasher::new(TAG_SIG_SELF_REF).string(name).finish();
    }
    match kt {
        KType::List { element, .. } => DigestHasher::new(TAG_LIST)
            .digest(canonical_type_digest(element, schema))
            .finish(),
        KType::Dict { key, value, .. } => DigestHasher::new(TAG_DICT)
            .digest(canonical_type_digest(key, schema))
            .digest(canonical_type_digest(value, schema))
            .finish(),
        KType::Record { fields, .. } => {
            let mut h = DigestHasher::new(TAG_RECORD);
            feed_record_canonical(&mut h, fields, schema);
            h.finish()
        }
        KType::KFunction { params, ret, .. } => {
            let mut h = DigestHasher::new(TAG_KFUNCTION);
            feed_record_canonical(&mut h, params, schema);
            h.digest(canonical_type_digest(ret, schema)).finish()
        }
        KType::Union { members, .. } => {
            let mut member_digests: Vec<TypeDigest> = members
                .iter()
                .map(|m| canonical_type_digest(m, schema))
                .collect();
            member_digests.sort_unstable();
            let mut h = DigestHasher::new(TAG_UNION);
            h.count(member_digests.len());
            for d in member_digests {
                h.digest(d);
            }
            h.finish()
        }
        KType::ConstructorApply { ctor, args, .. } => {
            let mut h = DigestHasher::new(TAG_CONSTRUCTOR_APPLY);
            h.digest(canonical_type_digest(ctor, schema));
            feed_record_canonical(&mut h, args, schema);
            h.finish()
        }
        _ => kt.digest(),
    }
}

/// `Some(member name)` iff `kt` references one of `schema`'s own abstract members — an
/// `AbstractType` sourced at the schema's `sig_id`, of either order (a first-order slot type, or
/// the ctor position of a `ConstructorApply` / a bare higher-kinded slot). The shape
/// `substitute_sig_members` rewrites. `None` for a self-sig (no `sig_id`, no abstract members), so
/// a self-sig's member digests are its content unchanged — including any generative
/// `AbstractType` mints, which correctly stay id-keyed.
fn schema_self_ref<'k>(kt: &'k KType, schema: &SigSchema) -> Option<&'k str> {
    match kt {
        KType::AbstractType { source, name, .. } if schema.sig_id == Some(*source) => Some(name),
        _ => None,
    }
}

/// [`feed_record`] with each field type routed through [`canonical_type_digest`] — the canonical
/// twin used inside a signature's schema-content walk.
fn feed_record_canonical(h: &mut DigestHasher, record: &Record<KType>, schema: &SigSchema) {
    let mut pairs: Vec<(&str, TypeDigest)> = record
        .iter()
        .map(|(name, value)| (name.as_str(), canonical_type_digest(value, schema)))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    h.count(pairs.len());
    for (name, d) in pairs {
        h.string(name).digest(d);
    }
}

/// A member reference's digest: `(set digest, index)` once the set is sealed.
fn set_ref_digest(set: &Rc<RecursiveSet>, index: usize) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SET_REF);
    feed_set_identity(&mut h, set);
    h.count(index).finish()
}

/// Feed a set's identity into `h`: its sealed digest, or — in the pre-seal window only — a
/// pointer-derived transient (see [`digest_of`]).
fn feed_set_identity(h: &mut DigestHasher, set: &Rc<RecursiveSet>) {
    match set.digest() {
        Some(d) => {
            h.byte(1).digest(d);
        }
        None => {
            h.byte(0).count(Rc::as_ptr(set) as *const () as usize);
        }
    }
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

/// A recursive set's digest, computed at seal (every member filled). Generative sets (opaque
/// ascription) fold their per-application nonce first, so two applications never unify; every
/// other set is content-only. Members are digested in declaration order; a member's identity
/// is `(name, kind, schema)` — `scope_id` is excluded so the same declaration elaborated
/// twice unifies. Intra-set sibling references are `SetLocal` (a bare index, tag-only), so
/// computing a set's digest never recurses into the set itself.
pub fn set_digest(set: &RecursiveSet) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_RECURSIVE_SET);
    match set.generative_nonce() {
        Some(nonce) => {
            h.byte(1).scope_id(nonce);
        }
        None => {
            h.byte(0);
        }
    }
    h.count(set.members().len());
    for member in set.members() {
        h.string(&member.name).byte(kkind_tag(member.kind));
        let borrow = member.schema();
        let schema = borrow
            .as_ref()
            .expect("set_digest computes at seal — every member is filled");
        match schema {
            NominalSchema::NewType(repr) => {
                h.byte(0).digest(repr.digest());
            }
            NominalSchema::TypeConstructor {
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
