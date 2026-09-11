//! `KType` — the handle naming one interned type, and the builtin vocabulary that lowers to a
//! fixed one.
//!
//! A `KType` *is* its type's content digest ([`TypeDigest`]): a bare `u128`, `Copy`, carrying no
//! pointer, no index, and no reference to the registry that minted it. Equality, hashing and
//! ordering derive on that one word, so comparing two types is comparing two integers and no
//! structural descent exists to fall back to. Content lives in a
//! [`TypeRegistry`](super::registry::TypeRegistry), keyed by the same digest, so any operation
//! that needs a type's shape — rendering, kind classification, the relations — takes the registry
//! and reads the [`TypeNode`].
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at [`KType::from_symbol`] time.

use crate::parse::{StaticName, TypeSymbol};

use super::digest::TypeDigest;
use super::kind::KKind;
use super::node::TypeNode;
use super::registry::TypeRegistry;

/// A handle to one interned type: the content digest of its [`TypeNode`], and nothing else.
///
/// Identity is the digest, so two independently built types with the same content are one handle
/// — that is the interning contract, not a coincidence of sharing. `Ord` is the numeric order of
/// the digest: meaningless as a type order, useful only for canonical sorting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KType(TypeDigest);

/// The fixed spellings of the singly-named builtin leaves — the one authority both
/// [`render`](super::render) and [`KType::name_symbol`] read, so the rendered text and the
/// classified symbol cannot drift apart. Each mints once per process at its first symbol read.
pub(super) static NUMBER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Number");
pub(super) static STR_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Str");
pub(super) static BOOL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Bool");
pub(super) static NULL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Null");
pub(super) static IDENTIFIER_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "Identifier");
pub(super) static NAME_TOKEN_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "NameToken");
pub(super) static TYPE_NAME_TOKEN_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "TypeNameToken");
pub(super) static KEXPRESSION_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "KExpression");
pub(super) static SIGILED_TYPE_EXPR_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "SigiledTypeExpr");
pub(super) static RECORD_TYPE_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "RecordType");
pub(super) static ANY_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Any");
pub(super) static NEVER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Never");
/// The empty signature's surface name — the `:Module` lattice top.
pub(super) static MODULE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Module");
static LIST_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "List");
static DICT_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Dict");
static SIGNATURE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Signature");

impl KType {
    // --- Fixed handles ---
    //
    // The twelve leaves, the five `OfKind` values, `List<Any>`, `Dict<Any, Any>` and the empty
    // signature name content every registry pre-seeds (`TypeRegistry::in_region`), so their digests are
    // known at compile time and lowering a builtin type name needs no registry in hand. The
    // literals below are the digest recipe's output; `constants_match_freshly_interned_nodes` in
    // the golden module recomputes each one from its own node, so a recipe change fails loudly
    // here rather than silently re-identifying a leaf.

    pub const NUMBER: KType = KType(TypeDigest(0xe21d67f1_7aa25f92_e072c1bb_1f72fc48));
    pub const STR: KType = KType(TypeDigest(0xda8a6add_c7627c0f_ae4be842_dfbe13ab));
    pub const BOOL: KType = KType(TypeDigest(0x01210944_fd6fb8f8_0c9ba36e_1de8e0e1));
    pub const NULL: KType = KType(TypeDigest(0xbc9d88bb_75d5fb35_a4fd343e_749a380c));
    pub const IDENTIFIER: KType = KType(TypeDigest(0x41b73c3e_2391bbb4_6b850e4f_e740cb84));
    /// A binder position taking a bare name of either class — never resolved, never lowered.
    pub const NAME_TOKEN: KType = KType(TypeDigest(0x7dec3e82_f44adbda_2f8cc4c2_47b790eb));
    /// A binder position taking a bare Type-class name — never resolved, never lowered.
    pub const TYPE_NAME_TOKEN: KType = KType(TypeDigest(0xb9978361_a0bb1460_82127faa_0711eeca));
    pub const KEXPRESSION: KType = KType(TypeDigest(0x63c296ef_dbe5d41c_9969ddda_6b0b311c));
    pub const SIGILED_TYPE_EXPR: KType = KType(TypeDigest(0xf6d652dc_848e0f69_4a152496_ddd88b44));
    pub const RECORD_TYPE: KType = KType(TypeDigest(0x387dfced_dc0a5d96_da3b29a5_dde0f32e));
    pub const ANY: KType = KType(TypeDigest(0xd9f70f99_49f95b5c_44d7ce99_10aa1972));
    /// The uninhabited bottom of the lattice — below every other type, admitted by no value, and
    /// the identity element of join and of union canonicalization.
    pub const NEVER: KType = KType(TypeDigest(0x59dd8c1f_71e395f4_77717ff5_a93c2600));

    pub const PROPER_TYPE: KType = KType(TypeDigest(0xe082d96a_231e2f4c_af1e256b_459a681f));
    pub const SIGNATURE_KIND: KType = KType(TypeDigest(0xa74d105b_68705a5a_4c93c325_b2bb4032));
    pub const ANY_TYPE: KType = KType(TypeDigest(0x6230fb6f_d4cb83ad_59072aad_08f93e54));
    pub const NEW_TYPE: KType = KType(TypeDigest(0x3079a661_6197d2a5_46103cc5_f0cbfeaa));
    pub const TYPE_CONSTRUCTOR: KType = KType(TypeDigest(0x1522ec89_d5fd3ca8_2db00c80_75beafb3));

    /// `List<Any>` — what the bare `List` name lowers to.
    pub const LIST_OF_ANY: KType = KType(TypeDigest(0x9d40af7c_078f46c4_bd4a8f94_98f5fd63));
    /// `Dict<Any, Any>` — what the bare `Dict` name lowers to.
    pub const DICT_ANY_ANY: KType = KType(TypeDigest(0xf9b9d64d_aa69edda_e7a59f82_4e0f5015));
    /// The empty signature — top of the module lattice, the type `:Module` lowers to. It
    /// constrains nothing, so every module value satisfies it.
    pub const EMPTY_SIGNATURE: KType = KType(TypeDigest(0xb80aaa8d_7e3507bd_e06a1496_5250ca90));

    /// The type-accepting slot admitting `kind` — one of the pre-seeded `OfKind` handles.
    pub const fn of_kind(kind: KKind) -> KType {
        match kind {
            KKind::ProperType => KType::PROPER_TYPE,
            KKind::Signature => KType::SIGNATURE_KIND,
            KKind::AnyType => KType::ANY_TYPE,
            KKind::NewType => KType::NEW_TYPE,
            KKind::TypeConstructor => KType::TYPE_CONSTRUCTOR,
        }
    }

    /// Wrap a digest as the handle naming it. Named rather than a public tuple field, and
    /// module-internal, so the wrapping is a deliberate act: `digest` must already be the digest
    /// of interned content, or a member handle derived from its component's digest.
    pub(super) const fn from_digest(digest: TypeDigest) -> KType {
        KType(digest)
    }

    /// This type's content digest — its identity, and its key in the registry's node table.
    pub const fn digest(self) -> TypeDigest {
        self.0
    }

    /// Look up a `KType` by the name a user can write in source (e.g. `Number`, `List`). Every
    /// name here lowers to a fixed handle, so the lookup needs no registry: the content each one
    /// names is pre-seeded into every registry at construction.
    pub fn from_symbol(name: TypeSymbol) -> Option<KType> {
        builtin_types()
            .into_iter()
            .find(|(declared, _)| declared.symbol() == name)
            .map(|(_, ktype)| ktype)
    }

    /// The bare Type token this type names itself by, as the classified symbol — or `None` for a
    /// type whose surface is compound syntax rather than one token.
    ///
    /// No text is hashed on any arm: a node that carries its declared name answers the symbol
    /// stored in it, and a builtin leaf answers its fixed spelling's [`StaticName`] memo — the same
    /// statics rendering reads, so the two doors agree by construction. Static spellings are
    /// recorded in the run's label interner here, matching what declaring the name from text would
    /// have recorded, so rendering resolves either way.
    pub fn name_symbol(
        self,
        types: &TypeRegistry<'_>,
        labels: &crate::parse::LabelInterner,
    ) -> Option<TypeSymbol> {
        let fixed = |name: &StaticName<TypeSymbol>| Some(labels.record(name));
        types.with_node(self, |node| match node {
            TypeNode::Number => fixed(&NUMBER_NAME),
            TypeNode::Str => fixed(&STR_NAME),
            TypeNode::Bool => fixed(&BOOL_NAME),
            TypeNode::Null => fixed(&NULL_NAME),
            TypeNode::Identifier => fixed(&IDENTIFIER_NAME),
            TypeNode::NameToken => fixed(&NAME_TOKEN_NAME),
            TypeNode::TypeNameToken => fixed(&TYPE_NAME_TOKEN_NAME),
            TypeNode::KExpression => fixed(&KEXPRESSION_NAME),
            TypeNode::SigiledTypeExpr => fixed(&SIGILED_TYPE_EXPR_NAME),
            TypeNode::RecordType => fixed(&RECORD_TYPE_NAME),
            TypeNode::Any => fixed(&ANY_NAME),
            TypeNode::Never => fixed(&NEVER_NAME),
            TypeNode::OfKind(kind) => Some(kind.surface_symbol(labels)),
            TypeNode::AbstractType { name, .. } => Some(*name),
            TypeNode::SetMember { name, .. } => Some(*name),
            TypeNode::Signature { schema_digest, .. } => (*schema_digest
                == super::digest::empty_schema_digest())
            .then(|| labels.record(&MODULE_NAME)),
            TypeNode::List { .. }
            | TypeNode::Dict { .. }
            | TypeNode::Record { .. }
            | TypeNode::KFunction { .. }
            | TypeNode::ExpressionShape { .. }
            | TypeNode::Quantified { .. }
            | TypeNode::DeferredReturn(_)
            | TypeNode::Union { .. }
            | TypeNode::ConstructorApply { .. }
            | TypeNode::Sibling(_) => None,
        })
    }

    /// Classify a *type* into its shallow dispatch [`KKind`] — the value-side direction of
    /// `OfKind`. A signature is `Signature`, a user-declared nominal is its family read off its
    /// member node, an abstract member with declared parameters is a constructor, and every other
    /// type is `ProperType`. Never returns [`KKind::AnyType`], which is a slot-only expectation.
    pub fn kind_of(self, types: &TypeRegistry<'_>) -> KKind {
        types.with_node(self, |node| match node {
            TypeNode::Signature { .. } => KKind::Signature,
            TypeNode::SetMember { kind, .. } => *kind,
            TypeNode::ConstructorApply { constructor, .. } => constructor.kind_of(types),
            TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => {
                KKind::TypeConstructor
            }
            _ => KKind::ProperType,
        })
    }
}

/// Every builtin type name beside the handle it lowers to, in registration order.
///
/// A bare `List` or `Dict` names its fully-general instance, and `Module` names the empty
/// signature — the surface spelling admits no parameters, so the handle it stands for is fixed.
/// `Never` names the lattice bottom, so a slot written `:Never` is legal and admits nothing.
///
/// Each name is a [`StaticName`], so its symbol is minted at first read and loaded thereafter:
/// seeding a second run's root re-registers the same twelve names without hashing a spelling.
pub fn builtin_types() -> [(&'static StaticName<TypeSymbol>, KType); 12] {
    [
        (&NUMBER_NAME, KType::NUMBER),
        (&STR_NAME, KType::STR),
        (&BOOL_NAME, KType::BOOL),
        (&NULL_NAME, KType::NULL),
        (&LIST_NAME, KType::LIST_OF_ANY),
        (&DICT_NAME, KType::DICT_ANY_ANY),
        (&KEXPRESSION_NAME, KType::KEXPRESSION),
        (&ANY_TYPE_NAME, KType::of_kind(KKind::AnyType)),
        (&MODULE_NAME, KType::EMPTY_SIGNATURE),
        (&SIGNATURE_NAME, KType::of_kind(KKind::Signature)),
        (&ANY_NAME, KType::ANY),
        (&NEVER_NAME, KType::NEVER),
    ]
}

/// The `:Type` surface — [`KKind::AnyType`]'s own spelling, named here so the builtin table and
/// the kind's `surface_symbol` mint one memo apiece rather than sharing one across two roles.
static ANY_TYPE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Type");

/// A handle prints as its digest and nothing else: rendering content would need a registry, which
/// a `Formatter`-only signature cannot reach, and the digest is the whole identity anyway.
impl std::fmt::Debug for KType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KType(0x{:032x})", self.0.0)
    }
}
