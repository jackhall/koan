//! `TypeDigest` — the wide content-hash that *is* a [`KType`]'s identity, and the one recipe that
//! produces one.
//!
//! Every type carries a digest computed bottom-up from its children (a recursive set at seal,
//! over its finite SCC presentation). Equality is one digest compare and hashing keys on the
//! digest; the width is chosen so an accidental collision is less likely than a hardware fault,
//! so digest equality is type equality with no repair path.
//!
//! The digest is a pure function of type content, so two independently built types with the same
//! content digest equal with no shared interner. Generativity is one explicit mechanism applied in
//! two places: a minted [`ScopeId`] nonce folded into the content ahead of everything else,
//! carried by a recursive-group window and by an abstract member.
//!
//! **There is one recipe.** A node's digest is its tag byte, its own scalar payload, and its
//! children's digests — which are already known, because children are handles. Nothing here walks
//! a type: [`node_digest`] is one layer deep, and [`schema_content_digest`] reads a schema's
//! members by their handles. Projection re-sources every reference to a signature's own abstract
//! members to [`ScopeId::SENTINEL`] before a schema reaches here, so a textually identical
//! declaration already presents identical handles and there is nothing left for a deep
//! canonicalizing walk to do.
//!
//! **The hasher lives here and only here.** Every payload begins with a distinct domain tag byte
//! so no two variants can share a digest, every `String` is length-prefixed so concatenation is
//! unambiguous, and every child digest / [`ScopeId`] / integer is fed little-endian.

use crate::memory::ScopeId;
use crate::parse::{Symbol, TypeSymbol};

use super::handle::KType;
use super::kind::KKind;
use super::node::{NodeSchema, TypeNode};
use super::operators::{FoldDirection, ReductionMode};
use super::record::Record;
use super::registry::TypeRegistry;
use super::schema::SigSchema;
use super::shape::{DeferredReturnSurface, DispatchTokenElement};
use smallvec::SmallVec;

/// A `KType`'s content identity: the low 128 bits of a BLAKE3 hash of its content.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TypeDigest(pub(super) u128);

/// The digest's hexadecimal spelling — the one reading a caller outside the lattice has of the
/// bits, for a diagnostic that names a handle by identity.
impl std::fmt::LowerHex for TypeDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::LowerHex::fmt(&self.0, f)
    }
}

// Domain tag bytes — one per digestible shape, so no two variants can share a digest even with
// identical trailing payloads. These values are identity-load-bearing: never reorder or reuse a
// retired one. The tag table is hand-written because the tags *are* the identity;
// `tag_table_covers_every_variant` in the golden module pins that every node kind has one.
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
// 0x12, 0x13, 0x15 and 0x18 are retired — never reuse them.
const TAG_SET_REF: u8 = 0x14;
const TAG_UNION: u8 = 0x16;
const TAG_SIGNATURE: u8 = 0x17;
const TAG_ABSTRACT_TYPE: u8 = 0x19;
const TAG_CONSTRUCTOR_APPLY: u8 = 0x1A;
const TAG_RECURSIVE_SET: u8 = 0x1B;
const TAG_SIG_CONTENT: u8 = 0x1C;
// 0x1D is retired: it tagged the deep canonicalizing walk's self-reference leaf, which no longer
// exists — projection's sentinel re-sourcing makes an own-member reference content-determined.
const TAG_NAME_TOKEN: u8 = 0x1E;
const TAG_TYPE_NAME_TOKEN: u8 = 0x1F;
const TAG_NEVER: u8 = 0x20;
const TAG_EXPRESSION_SHAPE: u8 = 0x21;
const TAG_QUANTIFIED: u8 = 0x22;

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

    /// A [`Symbol`], fixed-width: its 128 bits little-endian. No length prefix — every symbol is
    /// the same width, so the stream stays unambiguous without one.
    fn symbol(&mut self, symbol: Symbol) -> &mut Self {
        self.inner.update(&symbol.0.to_le_bytes());
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

/// Stable one-byte tag for a `KKind` — its own discriminant is unstable across enum reordering,
/// so map explicitly.
fn kkind_tag(k: KKind) -> u8 {
    match k {
        KKind::ProperType => 0,
        KKind::Signature => 1,
        KKind::AnyType => 2,
        KKind::NewType => 3,
        KKind::TypeConstructor => 4,
    }
}

/// The digest of a [`TypeNode`] — the identity of the type it interns as, and the key the registry
/// stores it under.
///
/// Two recipes are not derived from child handles. A [`TypeNode::Sibling`] is its bare index under
/// `TAG_SET_LOCAL`, meaningful only against an ambient window. A [`TypeNode::SetMember`] is
/// `(component digest, index in component)` — its schema is *not* re-fed here, because the
/// component digest was computed over exactly that content at seal.
pub(super) fn node_digest(node: &TypeNode) -> TypeDigest {
    match node {
        TypeNode::Number => leaf_digest(TAG_NUMBER),
        TypeNode::Str => leaf_digest(TAG_STR),
        TypeNode::Bool => leaf_digest(TAG_BOOL),
        TypeNode::Null => leaf_digest(TAG_NULL),
        TypeNode::Identifier => leaf_digest(TAG_IDENTIFIER),
        TypeNode::NameToken => leaf_digest(TAG_NAME_TOKEN),
        TypeNode::TypeNameToken => leaf_digest(TAG_TYPE_NAME_TOKEN),
        TypeNode::KExpression => leaf_digest(TAG_KEXPRESSION),
        TypeNode::SigiledTypeExpr => leaf_digest(TAG_SIGILED_TYPE_EXPR),
        TypeNode::RecordType => leaf_digest(TAG_RECORD_TYPE),
        TypeNode::Any => leaf_digest(TAG_ANY),
        TypeNode::Never => leaf_digest(TAG_NEVER),
        TypeNode::OfKind(k) => of_kind_digest(*k),
        TypeNode::DeferredReturn(surface) => deferred_return_digest(surface),
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            nonce,
            bound,
        } => abstract_type_digest(*source, *name, param_names, *nonce, *bound),
        TypeNode::List { element } => list_digest(element.digest()),
        TypeNode::Dict { key, value } => dict_digest(key.digest(), value.digest()),
        TypeNode::Record { fields } => record_digest(fields),
        TypeNode::KFunction { params, ret } => function_digest(params, ret.digest()),
        TypeNode::ExpressionShape {
            quantifiers,
            elements,
            ret,
        } => shape_digest(quantifiers.len(), elements, ret.digest()),
        TypeNode::Quantified { index, bound } => quantified_digest(*index, *bound),
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

/// A deferred FN return: a discriminant byte for the surface shape, then the shape's own identity
/// — a bare name's symbol bits, a captured expression's canonical render.
fn deferred_return_digest(surface: &DeferredReturnSurface) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_DEFERRED_RETURN);
    match surface {
        DeferredReturnSurface::Type(name) => h.byte(0).symbol(name.symbol()),
        DeferredReturnSurface::Expression(text) => h.byte(1).string(text),
    }
    .finish()
}

/// A relative sibling reference: its bare index under `TAG_SET_LOCAL`, so computing an enclosing
/// component's digest never recurses back into the component.
fn sibling_digest(index: usize) -> TypeDigest {
    DigestHasher::new(TAG_SET_LOCAL).count(index).finish()
}

/// A named rigid variable's identity fields: the generativity `nonce` first, then the binder
/// `source`, the name, the parameter names fed sorted so the encoding is order-blind, and the
/// bound the variable stands over. Names feed as fixed-width symbol bits and sort by those bits —
/// a canonical order over the same set.
fn abstract_type_digest(
    source: ScopeId,
    name: TypeSymbol,
    param_names: &[TypeSymbol],
    nonce: Option<ScopeId>,
    bound: KType,
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
    h.scope_id(source)
        .symbol(name.symbol())
        .count(param_names.len());
    let mut sorted: Vec<TypeSymbol> = param_names.to_vec();
    sorted.sort_unstable();
    for param in sorted {
        h.symbol(param.symbol());
    }
    h.digest(bound.digest()).finish()
}

// Per-shape digest builders. Each takes its children's handles — which are already their digests
// — so the work is shallow: one hash over one tag and a few `u128`s, never a walk.

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

/// An expression shape: its quantifier **arity** (never the names — alpha-variants are one type),
/// then its element run in order, each element a keyword's symbol bits behind a `1` byte or a slot
/// type's digest behind a `0`, then the return. Order is identity here where a `KFunction`'s
/// parameter record is order-blind: a shape's argument positions are what dispatch reads. Each
/// variable's bound rides in its own `Quantified` occurrences, which canonical form guarantees.
///
/// Reachable from the registry so a shape can be interned probe-first: the `ExpressionShape` arm
/// of [`node_digest`] is this call over the node's own fields, so a digest taken here off a
/// borrowed element run equals the digest of the node that run would build.
pub(super) fn shape_digest(
    arity: usize,
    elements: &[DispatchTokenElement],
    ret: TypeDigest,
) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_EXPRESSION_SHAPE);
    h.count(arity).count(elements.len());
    for element in elements {
        match element {
            DispatchTokenElement::Keyword(symbol) => h.byte(1).symbol(symbol.symbol()),
            DispatchTokenElement::Slot(kt) => h.byte(0).digest(kt.digest()),
        };
    }
    h.digest(ret).finish()
}

/// A positional rigid variable: its index in the enclosing shape's quantifier group, then the
/// bound it stands over. The bound is identity — two shapes differing only in a variable's bound
/// admit different arguments.
fn quantified_digest(index: usize, bound: KType) -> TypeDigest {
    DigestHasher::new(TAG_QUANTIFIED)
        .count(index)
        .digest(bound.digest())
        .finish()
}

/// A union — order-blind, matching its set-based identity: sort the member digests.
///
/// Reachable from the registry so a union can be interned probe-first: the `Union` arm of
/// [`node_digest`] is this call over the node's member slice.
pub(super) fn union_digest(members: &[KType]) -> TypeDigest {
    // The sort needs its own buffer, and the digest is taken per union evaluation rather than once
    // at intern, so the members ride inline: the width a union is written at fits.
    let mut member_digests: SmallVec<[TypeDigest; 8]> =
        members.iter().map(|m| m.digest()).collect();
    member_digests.sort_unstable();
    let mut h = DigestHasher::new(TAG_UNION);
    h.count(member_digests.len());
    for d in member_digests {
        h.digest(d);
    }
    h.finish()
}

/// `ConstructorApply(ctor, args)` — the args feed symbol-keyed and symbol-sorted (see
/// [`feed_record`]), matching the order-blind identity of the args `Record`.
fn constructor_apply_digest(ctor: TypeDigest, args: &Record<KType>) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_CONSTRUCTOR_APPLY);
    h.digest(ctor);
    feed_record(&mut h, args);
    h.finish()
}

/// A module-signature type's digest: its schema's content digest — identity by interface, not by
/// mint. `WITH` pins fold into the schema before interning, so the schema content is the whole
/// identity.
fn signature_digest(content_digest: TypeDigest) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIGNATURE);
    h.digest(content_digest);
    h.finish()
}

/// The content digest of a normalized signature schema — a pure function of its members, read one
/// layer deep.
///
/// Abstract members feed `(name, order, sorted parameter names, bound digest)`; manifest members,
/// value slots and keyworded shapes feed their handles' own digests, the named groups in
/// symbol-sorted order (the maps are unordered) and the keyworded group in the schema's canonical
/// order; operator records feed their member runs and modes last.
///
/// No walk descends a member. Projection has already re-sourced every reference to one of the
/// signature's own abstract members to [`ScopeId::SENTINEL`], so a textually identical declaration
/// projects to identical handles and a schema digests by its member handles alone.
pub(super) fn schema_content_digest(schema: &SigSchema, types: &TypeRegistry) -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIG_CONTENT);

    // Each abstract member feeds its name, then its order — `0x00` for a first-order proper type,
    // `0x01` plus the parameter names for a constructor — then the bound it stands over. The
    // parameter names feed sorted, so the encoding is order-blind.
    let mut abstracts: Vec<(TypeSymbol, Vec<TypeSymbol>, KType)> = schema
        .abstract_members
        .iter()
        .map(|(name, kt)| {
            let (params, bound) = read_abstract(*kt, types);
            (*name, params, bound)
        })
        .collect();
    abstracts.sort_by_key(|(name, _, _)| *name);
    h.count(abstracts.len());
    for (name, mut param_names, bound) in abstracts {
        h.symbol(name.symbol());
        if param_names.is_empty() {
            h.byte(0);
        } else {
            param_names.sort_unstable();
            h.byte(1).count(param_names.len());
            for param in param_names {
                h.symbol(param.symbol());
            }
        }
        h.digest(bound.digest());
    }

    feed_named_types(
        &mut h,
        schema
            .manifest_members
            .iter()
            .map(|(n, kt)| (n.symbol(), *kt)),
    );
    feed_named_types(
        &mut h,
        schema.value_slots.iter().map(|(n, kt)| (n.symbol(), *kt)),
    );

    // Keyworded members: each declared shape's own digest, in the schema's canonical member order.
    // The bucket key rides inside the shape's recipe, so it is fed once rather than beside each
    // member.
    h.count(schema.keyworded.len());
    for member in &schema.keyworded {
        h.digest(member.digest());
    }

    // Operator members: each declared chaining record as its member run then its mode. The channel
    // is stored canonically, so the declaration order never reaches the digest. A pairwise mode
    // additionally feeds its combiner symbol and direction — two records differing only in how
    // they chain are two interfaces.
    h.count(schema.operators.len());
    for group in &schema.operators {
        h.count(group.members.len());
        for member in &group.members {
            h.symbol(member.symbol());
        }
        match group.mode {
            ReductionMode::Unary => h.byte(0),
            ReductionMode::FoldLeft => h.byte(1),
            ReductionMode::FoldRight => h.byte(2),
            ReductionMode::Pairwise {
                combiner,
                direction,
            } => h.byte(3).symbol(combiner.symbol()).byte(match direction {
                FoldDirection::Left => 0,
                FoldDirection::Right => 1,
            }),
        };
    }
    h.finish()
}

/// The digest of the member-free schema — the module-lattice top (`:Module`), the type a
/// module-accepting slot lowers to. Byte-for-byte what [`schema_content_digest`] produces for an
/// empty [`SigSchema`], and computable without one because an empty schema names no member.
pub(super) fn empty_schema_digest() -> TypeDigest {
    let mut h = DigestHasher::new(TAG_SIG_CONTENT);
    h.count(0); // abstract_members
    h.count(0); // manifest_members (feed_named_types header)
    h.count(0); // value_slots (feed_named_types header)
    h.count(0); // keyworded members
    h.count(0); // operator records
    h.finish()
}

/// An abstract member's order and bound, read off its own node. The one read
/// [`schema_content_digest`] takes: a member handle names an `AbstractType`, whose parameter names
/// carry its order and whose `bound` is what it stands over. Anything else in the map is a
/// first-order member over `Any`.
fn read_abstract(member: KType, types: &TypeRegistry) -> (Vec<TypeSymbol>, KType) {
    // Owns: the parameter list feeds the sorted digest-input vector being built past this read.
    types.with_node(member, |node| match node {
        TypeNode::AbstractType {
            param_names, bound, ..
        } => (param_names.clone(), *bound),
        _ => (Vec::new(), KType::ANY),
    })
}

/// Feed a `name -> type` member sequence into `h` in symbol-sorted order (the source maps are
/// unordered), each type by its handle's own digest. Shared by the manifest members (Type-keyed)
/// and the value slots (value-keyed), which is why it takes raw [`Symbol`]s rather than one
/// classified key type.
fn feed_named_types(h: &mut DigestHasher, members: impl Iterator<Item = (Symbol, KType)>) {
    let mut pairs: Vec<(Symbol, TypeDigest)> =
        members.map(|(name, kt)| (name, kt.digest())).collect();
    pairs.sort_by_key(|(name, _)| *name);
    h.count(pairs.len());
    for (name, d) in pairs {
        h.symbol(name).digest(d);
    }
}

/// A sealed member's identity: its strongly-connected component's digest plus its index in that
/// component's canonical (member-name) order. The single derivation of a member handle — the seal
/// mints one per member, and every later consumer that knows a component recomputes the same value
/// rather than storing a sibling list.
///
/// The `byte(1)` is a fixed prefix of the recipe, not a discriminant: nothing pre-seal is
/// digestible, so there is no second arm to distinguish.
pub(super) fn member_ref_digest(scc_digest: TypeDigest, index: usize) -> TypeDigest {
    DigestHasher::new(TAG_SET_REF)
        .byte(1)
        .digest(scc_digest)
        .count(index)
        .finish()
}

/// Order-blind record digest: `(symbol, field digest)` pairs in canonical order — the numeric order
/// of the symbols. Matches `Record`'s order-blind equality. Shared by `Record` and `KFunction`
/// params and `ConstructorApply` arguments.
fn feed_record(h: &mut DigestHasher, record: &Record<KType>) {
    let mut pairs: Vec<(Symbol, TypeDigest)> = record
        .iter()
        .map(|(key, value)| (key.symbol(), value.digest()))
        .collect();
    pairs.sort_unstable_by_key(|pair| pair.0);
    h.count(pairs.len());
    for (symbol, d) in pairs {
        h.symbol(symbol).digest(d);
    }
}

/// One member as the component recipe presents it: its name, its kind, and its schema with every
/// sibling handle already re-encoded — intra-component references as a relative
/// [`TypeNode::Sibling`] into the component's own canonical order, cross-component references as
/// the referent's finished member handle.
pub(super) struct ComponentMember<'m> {
    pub name: TypeSymbol,
    pub kind: KKind,
    pub schema: &'m NodeSchema,
}

/// The content digest of one strongly-connected component of a recursive-group window — the
/// identity half of every member handle it contains (see [`member_ref_digest`]).
///
/// `members` arrive in the component's canonical order — the numeric order of their name symbols —
/// so two independently declared components with the same content present identically whatever
/// order they were written in. A generative component folds its nonce first, so two applications
/// never unify. Intra-component sibling references digest as bare relative indices, so computing a
/// component's digest never recurses back into the component.
///
/// A singleton component is byte-identical to the whole-declaration recipe it generalizes: count
/// `1`, one member, its own self-reference relative index `0`.
pub(super) fn component_digest(
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
        h.symbol(member.name.symbol()).byte(kkind_tag(member.kind));
        match member.schema {
            NodeSchema::NewType(repr) => {
                h.byte(0).digest(repr.digest());
            }
            NodeSchema::TypeConstructor {
                schema,
                param_names,
            } => {
                // HashMap iteration order is nondeterministic — sort by the keys' symbol bits.
                let mut entries: Vec<(TypeSymbol, TypeDigest)> =
                    schema.iter().map(|(k, v)| (*k, v.digest())).collect();
                entries.sort_unstable_by_key(|(name, _)| *name);
                h.byte(1).count(entries.len());
                for (name, d) in entries {
                    h.symbol(name.symbol()).digest(d);
                }
                h.count(param_names.len());
                for p in param_names {
                    h.symbol(p.symbol());
                }
            }
        }
    }
    h.finish()
}
