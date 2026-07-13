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
//! `generative_nonce`) and the id-keyed leaves (`Signature` / `Module` / `AbstractType`) —
//! so distinct abstractions stay distinct; the order-independence property is scoped to
//! types without such minted leaves.
//!
//! **The hasher lives here and only here.** Swapping the hash function or width is a
//! one-file change (the roadmap marks the exact function open, recommending 128-bit
//! truncated BLAKE3, which this ships). Every payload begins with a distinct domain tag
//! byte so no two variants can share a digest, every `String` is length-prefixed so
//! concatenation is unambiguous, and every child digest / `ScopeId` / integer is fed
//! little-endian.

use std::rc::Rc;

use crate::machine::core::ScopeId;

use super::kkind::KKind;
use super::ktype::{KType, SigSource};
use super::record::Record;
use super::recursive_set::{NominalSchema, RecursiveSet};
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
const TAG_KFUNCTOR: u8 = 0x0F;
const TAG_DEFERRED_RETURN: u8 = 0x10;
const TAG_SET_LOCAL: u8 = 0x11;
const TAG_RECURSIVE_REF: u8 = 0x12;
const TAG_UNRESOLVED: u8 = 0x13;
const TAG_SET_REF: u8 = 0x14;
const TAG_RECURSIVE_GROUP: u8 = 0x15;
const TAG_UNION: u8 = 0x16;
const TAG_SIGNATURE: u8 = 0x17;
const TAG_MODULE: u8 = 0x18;
const TAG_ABSTRACT_TYPE: u8 = 0x19;
const TAG_CONSTRUCTOR_APPLY: u8 = 0x1A;
const TAG_RECURSIVE_SET: u8 = 0x1B;

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
        KKind::Module => 1,
        KKind::Signature => 2,
        KKind::AnyType => 3,
        KKind::NewType => 4,
        KKind::TypeConstructor => 5,
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
        | KType::KFunctor { digest, .. }
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
        KType::Module { module } => module_digest(module.scope_id()),
        KType::AbstractType { source, name } => DigestHasher::new(TAG_ABSTRACT_TYPE)
            .scope_id(source.scope_id())
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

/// A functor type `(params) -> ret` — `body` is identity-inert and never reaches the digest.
pub(crate) fn functor_digest(params: &Record<KType>, ret: TypeDigest) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_KFUNCTOR);
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

/// `ConstructorApply(ctor, args)` — positional over `args`.
pub(crate) fn constructor_apply_digest(ctor: TypeDigest, args: &[KType]) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_CONSTRUCTOR_APPLY);
    h.digest(ctor);
    h.count(args.len());
    for a in args {
        h.digest(a.digest());
    }
    h.finish()
}

/// A module-signature type — positional over `pinned_slots` (matching `PartialEq`; do NOT
/// sort).
pub(crate) fn signature_digest(sig: SigSource, pinned_slots: &[(String, KType)]) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIGNATURE);
    h.scope_id(sig.sig_id());
    h.count(pinned_slots.len());
    for (name, kt) in pinned_slots {
        h.string(name).digest(kt.digest());
    }
    h.finish()
}

/// A module value's identity digest — `KType::Module`'s digest, and the subject/candidate key
/// the [`type_memos`](super::type_memos) `SigSatisfies` relation uses for a module's self-sig
/// side. Shared with [`digest_of`]'s `Module` arm so the two can never diverge.
pub(crate) fn module_digest(id: ScopeId) -> TypeDigest {
    DigestHasher::new(TAG_MODULE).scope_id(id).finish()
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
/// `Record`, `KFunction` params, and `KFunctor` params.
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
