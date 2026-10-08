//! The handles naming interned types: the raw [`Handle`], and the typed handles over it that say
//! what a type may hold — [`KType`] a concrete type, [`Parametric`] one that may hold a variable,
//! [`Scheme`] a quantified callable's type, [`DeclaredType`] either a type or a scheme — plus the
//! builtin vocabulary that lowers to a fixed handle.
//!
//! A `Handle` *is* its type's content digest ([`TypeDigest`]): a bare `u128`, `Copy`, carrying no
//! pointer, no index, and no reference to the registry that minted it. Equality, hashing and
//! ordering derive on that one word, so comparing two types is comparing two integers and no
//! structural descent exists to fall back to. Content lives in a
//! [`TypeRegistry`](super::registry::TypeRegistry), keyed by the same digest, so any operation
//! that needs a type's shape — rendering, kind classification, the relations — takes the registry
//! and reads the [`TypeNode`].
//!
//! A typed handle wraps a `Handle` and adds nothing at run time; what it adds is a promise the
//! compiler keeps. Only the lattice wraps a `Handle` into one ([`sealed::Wrap`]), so a `KType` is
//! concrete because every door that yields one builds it from concrete parts or checks it
//! ([`TypeRegistry::concrete`](super::registry::TypeRegistry::concrete)). **Concrete** means that,
//! outside a sealed `Signature` or `SetMember` node, the type holds no free `Quantified`, no
//! `Lexical`, no head `Parameter` and no quantified binder. An opaque carrier is concrete.
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at [`KType::from_symbol`] time.

use std::fmt;
use std::hash::Hash;

use crate::symbols::{StaticName, TypeSymbol};

use super::digest::TypeDigest;
use super::kind::KKind;
use super::node::TypeNode;
use super::registry::TypeRegistry;

/// A handle to one interned type: the content digest of its [`TypeNode`], and nothing else.
/// Identity only — every relation and door outside the lattice takes a typed handle.
///
/// Identity is the digest, so two independently built types with the same content are one handle
/// — that is the interning contract, not a coincidence of sharing. `Ord` is the numeric order of
/// the digest: meaningless as a type order, useful only for canonical sorting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(TypeDigest);

/// A **concrete** type: outside sealed content it holds no variable and no quantified binder. The
/// lattice's order, join and meet relate these and nothing else, and every value but a quantified
/// callable carries one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KType(Handle);

/// A type that may hold a variable — a free `Quantified`, a `Lexical` or a head `Parameter` —
/// but no quantified binder outside sealed content. Related by *fits* and the solver, never by
/// the order. Becomes a [`KType`] only by substitution, or through
/// [`TypeRegistry::concrete`](super::registry::TypeRegistry::concrete).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Parametric(Handle);

/// A quantified callable's type: a function type or an expression shape over a non-empty
/// `FOR ALL` group. The only parametric type a value carries and a call instantiates; its
/// positions read through [`TypeRegistry::scheme_node`](super::registry::TypeRegistry::scheme_node)
/// as [`Parametric`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Scheme(Handle);

/// What a callable, a registered shape, a signature member or a function value is typed by: a
/// type, or a quantified callable's [`Scheme`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DeclaredType<T> {
    Type(T),
    Scheme(Scheme),
}

pub(super) mod sealed {
    /// Wrapping a raw handle into a typed one: the lattice's alone, so a typed handle's promise is
    /// the lattice's to keep.
    pub trait Wrap: Copy {
        fn wrap(raw: super::Handle) -> Self;
    }
}

/// A handle a node can be read through and a door can build from: its children come back as
/// [`Child`](Self::Child) handles. Sealed — only the lattice wraps a raw handle.
pub trait TypeHandle: sealed::Wrap + Eq + Hash + Ord + fmt::Debug {
    /// What a node read through this handle hands its children back as.
    type Child: TypeHandle;
    fn raw(self) -> Handle;
}

impl TypeHandle for Handle {
    type Child = Handle;
    fn raw(self) -> Handle {
        self
    }
}

impl TypeHandle for KType {
    type Child = KType;
    fn raw(self) -> Handle {
        self.0
    }
}

impl TypeHandle for Parametric {
    type Child = Parametric;
    fn raw(self) -> Handle {
        self.0
    }
}

impl sealed::Wrap for Handle {
    fn wrap(raw: Handle) -> Self {
        raw
    }
}

impl sealed::Wrap for KType {
    fn wrap(raw: Handle) -> Self {
        KType(raw)
    }
}

impl sealed::Wrap for Parametric {
    fn wrap(raw: Handle) -> Self {
        Parametric(raw)
    }
}

impl sealed::Wrap for Scheme {
    fn wrap(raw: Handle) -> Self {
        Scheme(raw)
    }
}

/// `raw` as the typed handle `H` — the one wrapping door, for the lattice's own use. The caller
/// answers for `H`'s promise.
pub(super) fn wrap<H: sealed::Wrap>(raw: Handle) -> H {
    <H as sealed::Wrap>::wrap(raw)
}

impl Handle {
    /// Wrap a digest as the handle naming it. Named rather than a public tuple field, and
    /// module-internal, so the wrapping is a deliberate act: `digest` must already be the digest
    /// of interned content, or a member handle derived from its component's digest.
    pub(super) const fn from_digest(digest: TypeDigest) -> Handle {
        Handle(digest)
    }

    /// This type's content digest — its identity, and its key in the registry's node table.
    pub const fn digest(self) -> TypeDigest {
        self.0
    }

    /// Handle equality in `const` context.
    pub(super) const fn same_as(self, other: Handle) -> bool {
        self.0.0 == other.0.0
    }

    /// The code kind directly above this one in the code family's tree, or `None` for a handle that
    /// is no code kind below `Code`. A smaller syntax lies under a larger one wherever it can stand
    /// in its place; see [README.md](README.md) § The code family.
    pub(super) const fn code_parent(self) -> Option<Handle> {
        let parent = if self.same_as(Handle::BLOCK) {
            Handle::ANY_CODE
        } else if self.same_as(Handle::EXPRESSION) {
            Handle::BLOCK
        } else if self.same_as(Handle::DECLARATION)
            || self.same_as(Handle::LITERAL)
            || self.same_as(Handle::SYMBOL)
            || self.same_as(Handle::SIGILED_TYPE_EXPR)
            || self.same_as(Handle::RECORD_TYPE)
        {
            Handle::EXPRESSION
        } else if self.same_as(Handle::BINDER) {
            Handle::DECLARATION
        } else if self.same_as(Handle::NAME) || self.same_as(Handle::KEYWORD) {
            Handle::SYMBOL
        } else if self.same_as(Handle::IDENTIFIER) || self.same_as(Handle::TYPE_NAME_TOKEN) {
            Handle::NAME
        } else {
            return None;
        };
        Some(parent)
    }

    /// Whether this code kind lies at or under `kind` in the code family's tree — the walk up
    /// [`Self::code_parent`]. A handle that is no code kind is within only itself.
    pub(super) const fn within_code(self, kind: Handle) -> bool {
        let mut at = self;
        loop {
            if at.same_as(kind) {
                return true;
            }
            match at.code_parent() {
                Some(parent) => at = parent,
                None => return false,
            }
        }
    }
}

impl KType {
    /// This type's content digest — its identity.
    pub const fn digest(self) -> TypeDigest {
        self.0.0
    }

    /// Handle equality in `const` context — the one digest word compared. Derived `PartialEq` is
    /// not `const`, and a `static` table of slot types is checked against the code types where it
    /// is built.
    pub const fn same_as(self, other: KType) -> bool {
        self.0.same_as(other.0)
    }

    /// The code kind directly above this one in the code family's tree, or `None` for a type that
    /// is no code kind below `Code`; see [README.md](README.md) § The code family.
    pub const fn code_parent(self) -> Option<KType> {
        match self.0.code_parent() {
            Some(parent) => Some(KType(parent)),
            None => None,
        }
    }

    /// Whether this code kind lies at or under `kind` in the code family's tree. A type that is no
    /// code kind is within only itself.
    pub const fn within_code(self, kind: KType) -> bool {
        self.0.within_code(kind.0)
    }
}

impl Parametric {
    /// This type's content digest — its identity.
    pub const fn digest(self) -> TypeDigest {
        self.0.0
    }
}

impl Scheme {
    /// This type's content digest — its identity.
    pub const fn digest(self) -> TypeDigest {
        self.0.0
    }

    /// The raw handle — the lattice's alone, so no scheme reaches a door or a binding as a type.
    pub(super) fn raw(self) -> Handle {
        self.0
    }
}

/// Every concrete type may stand where a type may hold a variable.
impl From<KType> for Parametric {
    fn from(kt: KType) -> Self {
        Parametric(kt.0)
    }
}

impl From<KType> for DeclaredType<KType> {
    fn from(kt: KType) -> Self {
        DeclaredType::Type(kt)
    }
}

impl From<KType> for DeclaredType<Parametric> {
    fn from(kt: KType) -> Self {
        DeclaredType::Type(kt.into())
    }
}

impl From<Parametric> for DeclaredType<Parametric> {
    fn from(kt: Parametric) -> Self {
        DeclaredType::Type(kt)
    }
}

impl From<Scheme> for DeclaredType<KType> {
    fn from(scheme: Scheme) -> Self {
        DeclaredType::Scheme(scheme)
    }
}

impl From<Scheme> for DeclaredType<Parametric> {
    fn from(scheme: Scheme) -> Self {
        DeclaredType::Scheme(scheme)
    }
}

impl From<DeclaredType<KType>> for DeclaredType<Parametric> {
    fn from(declared: DeclaredType<KType>) -> Self {
        declared.map(Parametric::from)
    }
}

impl<T> DeclaredType<T> {
    /// The type, or `None` for a scheme.
    pub fn as_type(self) -> Option<T> {
        match self {
            DeclaredType::Type(kt) => Some(kt),
            DeclaredType::Scheme(_) => None,
        }
    }

    /// The scheme, or `None` for a type.
    pub fn as_scheme(self) -> Option<Scheme> {
        match self {
            DeclaredType::Type(_) => None,
            DeclaredType::Scheme(scheme) => Some(scheme),
        }
    }

    /// The type mapped through `f`; a scheme stays a scheme.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> DeclaredType<U> {
        match self {
            DeclaredType::Type(kt) => DeclaredType::Type(f(kt)),
            DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
        }
    }
}

impl<T: TypeHandle> DeclaredType<T> {
    /// The raw handle either arm names — identity only, and the lattice's alone.
    pub(super) fn raw(self) -> Handle {
        match self {
            DeclaredType::Type(kt) => kt.raw(),
            DeclaredType::Scheme(scheme) => scheme.raw(),
        }
    }

    /// This type's content digest — its identity.
    pub fn digest(self) -> TypeDigest {
        self.raw().digest()
    }
}

/// The fixed spellings of the singly-named builtin leaves — the one authority both
/// [`render`](super::render) and [`KType::name_symbol`] read, so the rendered text and the
/// classified symbol cannot drift apart. Each mints once per process at its first symbol read.
pub(super) static NUMBER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Number");
pub(super) static STR_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Str");
pub(super) static BOOL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Bool");
pub(super) static NULL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Null");
pub(super) static IDENTIFIER_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "Identifier");
pub(super) static SYMBOL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Symbol");
pub(super) static TYPE_NAME_TOKEN_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "TypeNameToken");
pub(super) static EXPRESSION_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "Expression");
pub(super) static SIGILED_TYPE_EXPR_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "SigiledTypeExpr");
pub(super) static RECORD_TYPE_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "RecordType");
pub(super) static LITERAL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Literal");
pub(super) static BLOCK_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Block");
pub(super) static DECLARATION_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "Declaration");
pub(super) static BINDER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Binder");
pub(super) static NAME_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Name");
pub(super) static KEYWORD_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Keyword");
pub(super) static ANY_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Any");
pub(super) static VALUE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Value");
pub(super) static CODE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Code");
pub(super) static NEVER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Never");
/// The empty signature's surface name — the `:Module` lattice top.
pub(super) static MODULE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Module");
static LIST_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "List");
static DICT_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Dict");
static SIGNATURE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Signature");

/// The fixed spelling of a singly-named builtin leaf, or `None` for every other node — the one
/// table [`KType::name_symbol`] and [`render`](super::render) both read.
pub(super) fn leaf_name<H>(node: &TypeNode<'_, H>) -> Option<&'static StaticName<TypeSymbol>> {
    Some(match node {
        TypeNode::Number => &NUMBER_NAME,
        TypeNode::Str => &STR_NAME,
        TypeNode::Bool => &BOOL_NAME,
        TypeNode::Null => &NULL_NAME,
        TypeNode::Identifier => &IDENTIFIER_NAME,
        TypeNode::Symbol => &SYMBOL_NAME,
        TypeNode::TypeNameToken => &TYPE_NAME_TOKEN_NAME,
        TypeNode::Expression => &EXPRESSION_NAME,
        TypeNode::SigiledTypeExpr => &SIGILED_TYPE_EXPR_NAME,
        TypeNode::RecordType => &RECORD_TYPE_NAME,
        TypeNode::Literal => &LITERAL_NAME,
        TypeNode::Block => &BLOCK_NAME,
        TypeNode::Declaration => &DECLARATION_NAME,
        TypeNode::Binder => &BINDER_NAME,
        TypeNode::Name => &NAME_NAME,
        TypeNode::Keyword => &KEYWORD_NAME,
        TypeNode::Any => &ANY_NAME,
        TypeNode::AnyValue => &VALUE_NAME,
        TypeNode::AnyCode => &CODE_NAME,
        TypeNode::Never => &NEVER_NAME,
        TypeNode::OfKind(_)
        | TypeNode::CodeNeeding { .. }
        | TypeNode::Parameter { .. }
        | TypeNode::Carrier { .. }
        | TypeNode::SetMember { .. }
        | TypeNode::Signature { .. }
        | TypeNode::List { .. }
        | TypeNode::Dict { .. }
        | TypeNode::Record { .. }
        | TypeNode::KFunction { .. }
        | TypeNode::ExpressionShape { .. }
        | TypeNode::Quantified { .. }
        | TypeNode::Lexical { .. }
        | TypeNode::DeferredReturn(_)
        | TypeNode::Union { .. }
        | TypeNode::ConstructorApply { .. }
        | TypeNode::SignatureApply { .. }
        | TypeNode::SignatureMeet { .. }
        | TypeNode::Sibling(_) => return None,
    })
}

/// Declares each fixed handle as a `KType` constant the rest of koan names.
macro_rules! fixed_handles {
    ($($(#[$doc:meta])* $name:ident = $digest:literal;)*) => {
        impl KType {
            $($(#[$doc])* pub const $name: KType = KType(Handle(TypeDigest($digest)));)*
        }
    };
}

/// The raw twin of each fixed handle the lattice's own code compares a raw handle against.
macro_rules! raw_twins {
    ($($name:ident),* $(,)?) => {
        impl Handle {
            $(pub(super) const $name: Handle = KType::$name.0;)*
        }
    };
}

// --- Fixed handles ---
//
// The twenty leaves, the five `OfKind` values, `List<Any>`, `Dict<Any, Any>`, the code
// composites, the empty record and the empty signature name content every registry pre-seeds
// (`TypeRegistry::in_region`), so their digests are known at compile time and lowering a
// builtin type name needs no registry in hand. The literals below are the digest recipe's
// output; `constants_match_freshly_interned_nodes` in the golden module recomputes each one
// from its own node, so a recipe change fails loudly here rather than silently re-identifying a
// leaf.
fixed_handles! {
    NUMBER = 0xe21d67f1_7aa25f92_e072c1bb_1f72fc48;
    STR = 0xda8a6add_c7627c0f_ae4be842_dfbe13ab;
    BOOL = 0x01210944_fd6fb8f8_0c9ba36e_1de8e0e1;
    NULL = 0xbc9d88bb_75d5fb35_a4fd343e_749a380c;
    /// A lone value name of code, under [`Self::NAME`].
    IDENTIFIER = 0x41b73c3e_2391bbb4_6b850e4f_e740cb84;
    /// A lone token of code: a name or a keyword.
    SYMBOL = 0x7dec3e82_f44adbda_2f8cc4c2_47b790eb;
    /// A lone type name of code, under [`Self::NAME`] — never resolved, never lowered.
    TYPE_NAME_TOKEN = 0xb9978361_a0bb1460_82127faa_0711eeca;
    /// One statement of code.
    EXPRESSION = 0x63c296ef_dbe5d41c_9969ddda_6b0b311c;
    SIGILED_TYPE_EXPR = 0xf6d652dc_848e0f69_4a152496_ddd88b44;
    RECORD_TYPE = 0x387dfced_dc0a5d96_da3b29a5_dde0f32e;
    /// A lone scalar literal or nested quote of code.
    LITERAL = 0xe0ba0587_757a04b5_57551481_dc141482;
    /// Statements of code: what every body slot takes.
    BLOCK = 0x30bf1d87_d3e4dc58_e64d3a9d_f3cfc6bb;
    /// One statement that declares a name or a shape.
    DECLARATION = 0x45826e93_1678c023_0193898d_a4337e86;
    /// One statement that declares and installs where it is written.
    BINDER = 0x33085145_8c8174df_bcb3e348_b6f3c6e2;
    /// A lone value or type name of code.
    NAME = 0x368aa850_9c281105_1746c919_0b150e96;
    /// A lone keyword of code.
    KEYWORD = 0x3ad3317d_c24be1d3_f80ceb6a_f4087539;
    ANY = 0xd9f70f99_49f95b5c_44d7ce99_10aa1972;
    /// The value family's top — what `Value` lowers to.
    ANY_VALUE = 0xf04a0d81_ff131a48_101bccdb_85dac271;
    /// The code family's top — what `Code` lowers to.
    ANY_CODE = 0x0f08f916_c60a7048_8e16cfb3_f68069e7;
    /// The uninhabited bottom of the lattice — below every other type, admitted by no value, and
    /// the identity element of join and of union canonicalization.
    NEVER = 0x59dd8c1f_71e395f4_77717ff5_a93c2600;
    PROPER_TYPE = 0xe082d96a_231e2f4c_af1e256b_459a681f;
    SIGNATURE_KIND = 0xa74d105b_68705a5a_4c93c325_b2bb4032;
    ANY_TYPE = 0x6230fb6f_d4cb83ad_59072aad_08f93e54;
    NEW_TYPE = 0x3079a661_6197d2a5_46103cc5_f0cbfeaa;
    TYPE_CONSTRUCTOR = 0x1522ec89_d5fd3ca8_2db00c80_75beafb3;
    /// `List<Any>` — what the bare `List` name lowers to.
    LIST_OF_ANY = 0x9d40af7c_078f46c4_bd4a8f94_98f5fd63;
    /// `Dict<Any, Any>` — what the bare `Dict` name lowers to.
    DICT_ANY_ANY = 0xf9b9d64d_aa69edda_e7a59f82_4e0f5015;
    /// The empty signature — top of the module lattice, the type `:Module` lowers to. It
    /// constrains nothing, so every module value satisfies it.
    EMPTY_SIGNATURE = 0x9d9c6ff8_d07721a9_90cd6da6_77262d30;
    /// `TypeNameToken | SigiledTypeExpr | RecordType` — the code a type is written as: a union's
    /// variant payload or a quantifier's bound. Not spellable.
    TYPE_CODE = 0xc41b235d_9ca37012_2069fcb7_39c1082e;
    /// `List<Name>` — a `FOR ALL` group or `FROM`'s field list.
    LIST_OF_NAME = 0xe4ef6471_b3309818_6e9fe04f_0c66f18a;
    /// `List<Expression>` — a `MODULE` or `GROUP` body's `OVER` list, each entry read as written.
    LIST_OF_EXPRESSION = 0x032c551d_dc7157af_ef4bf417_a34e1910;
    /// `List<Declaration>` — a `SIG` body, or the heads a bodyless `GROUP` declares.
    LIST_OF_DECLARATION = 0xdaf2c481_09b90725_35f0053e_594b6591;
    /// `Dict<Name, Block>` — a `MATCH … OVER` or `TRY` arm set, each guard a label.
    DICT_NAME_BLOCK = 0xd62f630b_16626d22_68df48ae_99aab59a;
    /// `Dict<TypeCode, Block>` — a `MATCH … WITH` arm set, each guard a type.
    DICT_TYPE_CODE_BLOCK = 0xf1066a0f_f0fe9f2f_9ba1351a_fb9f479f;
    /// `Dict<Name, TypeCode>` — a union's variants.
    DICT_NAME_TYPE_CODE = 0x34fd5145_6f54557d_3aeab867_9093541d;
    /// `List<Name> | Dict<Name, TypeCode>` — the code a `FOR ALL` group is written as: a list of
    /// names, or a dict of names to the code of their bounds.
    QUANTIFIER_CODE = 0xf2388522_88d6156a_8a8a6e23_acdbbf30;
    /// The empty record type — `FROM`'s argument and return.
    EMPTY_RECORD = 0xe7e914e1_0d893b27_988dbbdf_9ae2e427;
}

raw_twins! {
    NEVER, ANY, ANY_VALUE, ANY_CODE, ANY_TYPE, EMPTY_SIGNATURE, IDENTIFIER, SYMBOL,
    TYPE_NAME_TOKEN, EXPRESSION, SIGILED_TYPE_EXPR, RECORD_TYPE, LITERAL, BLOCK, DECLARATION,
    BINDER, NAME, KEYWORD, LIST_OF_NAME, DICT_NAME_TYPE_CODE,
}

impl KType {
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
    /// recorded in the run's symbol interner here, matching what declaring the name from text would
    /// have recorded, so rendering resolves either way.
    pub fn name_symbol(
        self,
        types: &TypeRegistry<'_>,
        symbols: &crate::symbols::SymbolInterner,
    ) -> Option<TypeSymbol> {
        let node = types.node(self);
        if let Some(name) = leaf_name(&node) {
            return Some(symbols.record(name));
        }
        match node {
            TypeNode::OfKind(kind) => Some(kind.surface_symbol(symbols)),
            TypeNode::Parameter { name, .. }
            | TypeNode::Carrier { name, .. }
            | TypeNode::SetMember { name, .. } => Some(name),
            TypeNode::Signature { .. } => {
                (self == KType::EMPTY_SIGNATURE).then(|| symbols.record(&MODULE_NAME))
            }
            _ => None,
        }
    }

    /// Classify a *type* into its shallow dispatch [`KKind`] — the value-side direction of
    /// `OfKind`. A signature type is `Signature`, a user-declared nominal is its family read off its
    /// member node, and every other type is `ProperType`. Never returns [`KKind::AnyType`], which is a slot-only expectation.
    pub fn kind_of(self, types: &TypeRegistry<'_>) -> KKind {
        match types.node(self) {
            TypeNode::Signature { .. }
            | TypeNode::SignatureApply { .. }
            | TypeNode::SignatureMeet { .. } => KKind::Signature,
            TypeNode::SetMember { kind, .. } => kind,
            TypeNode::ConstructorApply { constructor, .. } => constructor.kind_of(types),
            _ => KKind::ProperType,
        }
    }
}

/// Every builtin type name beside the handle it lowers to, in registration order.
///
/// A bare `List` or `Dict` names its fully-general instance, and `Module` names the empty
/// signature — the surface spelling admits no parameters, so the handle it stands for is fixed.
/// `Never` names the lattice bottom, so a slot written `:Never` is legal and admits nothing.
///
/// Each name is a [`StaticName`], so its symbol is minted at first read and loaded thereafter:
/// seeding a second run's root re-registers the same names without hashing a spelling.
pub fn builtin_types() -> [(&'static StaticName<TypeSymbol>, KType); 21] {
    [
        (&NUMBER_NAME, KType::NUMBER),
        (&STR_NAME, KType::STR),
        (&BOOL_NAME, KType::BOOL),
        (&NULL_NAME, KType::NULL),
        (&LIST_NAME, KType::LIST_OF_ANY),
        (&DICT_NAME, KType::DICT_ANY_ANY),
        (&EXPRESSION_NAME, KType::EXPRESSION),
        (&SYMBOL_NAME, KType::SYMBOL),
        (&LITERAL_NAME, KType::LITERAL),
        (&BLOCK_NAME, KType::BLOCK),
        (&DECLARATION_NAME, KType::DECLARATION),
        (&BINDER_NAME, KType::BINDER),
        (&NAME_NAME, KType::NAME),
        (&KEYWORD_NAME, KType::KEYWORD),
        (&ANY_TYPE_NAME, KType::of_kind(KKind::AnyType)),
        (&MODULE_NAME, KType::EMPTY_SIGNATURE),
        (&SIGNATURE_NAME, KType::of_kind(KKind::Signature)),
        (&ANY_NAME, KType::ANY),
        (&VALUE_NAME, KType::ANY_VALUE),
        (&CODE_NAME, KType::ANY_CODE),
        (&NEVER_NAME, KType::NEVER),
    ]
}

/// The `:Type` surface — [`KKind::AnyType`]'s own spelling, named here so the builtin table and
/// the kind's `surface_symbol` mint one memo apiece rather than sharing one across two roles.
static ANY_TYPE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Type");

/// A handle prints as its digest and nothing else: rendering content would need a registry, which
/// a `Formatter`-only signature cannot reach, and the digest is the whole identity anyway.
macro_rules! digest_debug {
    ($($handle:ident),*) => {$(
        impl fmt::Debug for $handle {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($handle), "(0x{:032x})"), self.digest().0)
            }
        }
    )*};
}

digest_debug!(Handle, KType, Parametric, Scheme);
