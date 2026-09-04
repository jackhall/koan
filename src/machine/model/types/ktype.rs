//! `KType` — the handle naming one interned type, attached to argument slots, function
//! return-types, and runtime values.
//!
//! A `KType` *is* its type's content digest ([`TypeDigest`]): a bare `u128`, `Copy`, carrying no
//! pointer, no index, and no reference to the registry that minted it. Equality, hashing and
//! ordering derive on that one word, so comparing two types is comparing two integers and no
//! structural descent exists to fall back to. Content lives in the run's
//! [`TypeRegistry`](super::registry::TypeRegistry), keyed by the same digest, so any operation
//! that needs a type's shape — rendering, kind classification, the predicates — takes the
//! registry and reads the [`TypeNode`].
//!
//! Container types are always parameterized: bare `List` / `Dict` lower to `List<Any>` /
//! `Dict<Any, Any>` at `from_symbol` time. There's no bare `KFunction` — "any function" with no
//! signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates live in `ktype_predicates.rs`; elaboration lives in `ktype_resolution.rs`.

use crate::machine::model::labels::{StaticName, Symbol, TypeSymbol};
use crate::machine::model::registries::RunRegistries;

use super::kkind::KKind;
use super::node::TypeNode;
use super::record::Record;
use super::registry::TypeRegistry;
use super::sig_schema::{SigSchema, render_keyworded_head, sorted_keyworded};
use super::type_digest::{TypeDigest, empty_schema_digest};
use smallvec::SmallVec;

/// A handle to one interned type: the content digest of its [`TypeNode`], and nothing else.
///
/// Identity is the digest, so two independently built types with the same content are one handle
/// — that is the interning contract, not a coincidence of sharing. `Ord` is the numeric order of
/// the digest: meaningless as a type order, useful only for canonical sorting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KType(TypeDigest);

/// The fixed spellings of the singly-named builtin leaves — the one authority both
/// [`KType::name`] and [`KType::name_symbol`] read, so the rendered text and the classified
/// symbol cannot drift apart. Each mints once per process at its first symbol read.
static NUMBER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Number");
static STR_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Str");
static BOOL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Bool");
static NULL_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Null");
static IDENTIFIER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Identifier");
static NAME_TOKEN_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "NameToken");
static TYPE_NAME_TOKEN_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "TypeNameToken");
static KEXPRESSION_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "KExpression");
static SIGILED_TYPE_EXPR_NAME: StaticName<TypeSymbol> =
    crate::static_name!(TypeSymbol, "SigiledTypeExpr");
static RECORD_TYPE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "RecordType");
static ANY_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Any");
static NEVER_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Never");
/// The empty signature's surface name — the `:Module` lattice top.
static MODULE_NAME: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Module");

impl KType {
    // --- Fixed handles ---
    //
    // The twelve leaves, the five `OfKind` values, `List<Any>`, `Dict<Any, Any>` and the empty
    // signature name content every registry pre-seeds (`TypeRegistry::new`), so their digests are
    // known at compile time and lowering a builtin type name needs no registry in hand. The
    // literals below are the digest recipe's output; `constants_match_freshly_interned_nodes`
    // in this module's tests recomputes each one from its own node, so a recipe change fails
    // loudly here rather than silently re-identifying a leaf.

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
    /// The uninhabited bottom of the lattice — more specific than every other type, admitted by
    /// no value, and the identity element of join and of union canonicalization.
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
    pub const EMPTY_SIGNATURE: KType = KType(TypeDigest(0x55ecc11f_39d3a140_69bc313b_aa2d1ed2));

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
    /// crate-internal, so the wrapping is a deliberate act: `digest` must already be the digest
    /// of interned content, or a member handle derived from its component's digest.
    pub(crate) const fn from_digest(digest: TypeDigest) -> KType {
        KType(digest)
    }

    /// This type's content digest — its identity, and its key in the registry's node table.
    pub const fn digest(self) -> TypeDigest {
        self.0
    }

    /// Surface-syntax rendering, straight into `f`. The rendered form parses back to the same
    /// type through the dispatch-driven type-language path (see
    /// [type-language via dispatch](../../../../design/typing/type-language-via-dispatch.md)).
    ///
    /// The one place the surface arms are written. Nodes are read in place and children recurse
    /// into the same formatter, so a nested type costs the caller's buffer and nothing else.
    pub fn write_name(
        self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        registries.types.with_node(self, |node| match node {
            TypeNode::Number => f.write_str(NUMBER_NAME.text()),
            TypeNode::Str => f.write_str(STR_NAME.text()),
            TypeNode::Bool => f.write_str(BOOL_NAME.text()),
            TypeNode::Null => f.write_str(NULL_NAME.text()),
            TypeNode::Identifier => f.write_str(IDENTIFIER_NAME.text()),
            TypeNode::NameToken => f.write_str(NAME_TOKEN_NAME.text()),
            TypeNode::TypeNameToken => f.write_str(TYPE_NAME_TOKEN_NAME.text()),
            TypeNode::KExpression => f.write_str(KEXPRESSION_NAME.text()),
            TypeNode::SigiledTypeExpr => f.write_str(SIGILED_TYPE_EXPR_NAME.text()),
            TypeNode::RecordType => f.write_str(RECORD_TYPE_NAME.text()),
            TypeNode::Any => f.write_str(ANY_NAME.text()),
            TypeNode::Never => f.write_str(NEVER_NAME.text()),
            TypeNode::OfKind(kind) => f.write_str(kind.surface_keyword()),
            TypeNode::List { element } => {
                f.write_str(":(LIST OF ")?;
                element.write_name(f, registries)?;
                f.write_str(")")
            }
            TypeNode::Dict { key, value } => {
                f.write_str(":(MAP ")?;
                key.write_name(f, registries)?;
                f.write_str(" -> ")?;
                value.write_name(f, registries)?;
                f.write_str(")")
            }
            // `:{x :Number y :Str}` — the braced type-sigil surface. Fields render
            // space-separated like FN params (the field-list parser accepts that).
            TypeNode::Record { fields } => {
                f.write_str(":{")?;
                write_param_record(f, fields, registries)?;
                f.write_str("}")
            }
            TypeNode::KFunction { params, ret } => {
                f.write_str(":(FN :{")?;
                write_param_record(f, params, registries)?;
                f.write_str("} -> ")?;
                ret.write_name(f, registries)?;
                f.write_str(")")
            }
            TypeNode::DeferredReturn(surface) => surface.write_surface(f, registries),
            // `:(A | B)` — members separated by ` | ` and wrapped in the type sigil. A compound
            // member already opens its own sigil (`:(LIST OF Number)`), which nests fine.
            TypeNode::Union { members } => {
                f.write_str(":(")?;
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" | ")?;
                    }
                    member.write_name(f, registries)?;
                }
                f.write_str(")")
            }
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                f.write_str(":(")?;
                constructor.write_name(f, registries)?;
                f.write_str(" {")?;
                for (index, (name, kt)) in arguments.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{} = ", display_label(name.symbol(), registries))?;
                    kt.write_name(f, registries)?;
                }
                f.write_str("})")
            }
            TypeNode::AbstractType { name, .. } => {
                write!(f, "{}", display_label(name.symbol(), registries))
            }
            // A sealed nominal member renders by its own member name — a bare newtype
            // (`:Wrapper`) or a per-variant member reached through its union (`:(Maybe.Some)`
            // yields the `Some` member, printed as `Some`).
            TypeNode::SetMember { name, .. } => {
                write!(f, "{}", display_label(name.symbol(), registries))
            }
            // A signature names itself by its content: the empty interface is the lattice top
            // `Module`, and any other interface renders its members structurally. There is no
            // declaration label to print — two textually identical `SIG` declarations are one
            // type, so naming either one of them would be a lie about the other.
            TypeNode::Signature {
                schema,
                schema_digest,
            } => {
                if *schema_digest == empty_schema_digest() {
                    f.write_str(MODULE_NAME.text())
                } else {
                    write_sig_schema(f, schema, registries)
                }
            }
            // Diagnostic only: a sibling reference is meaningful against its window and never
            // survives a seal, so nothing outside a mid-seal diagnostic can reach this.
            TypeNode::Sibling(index) => write!(f, "<sibling {index}>"),
        })
    }

    /// [`write_name`](Self::write_name) as a `Display` view — what a `format!` argument naming a
    /// type uses, so the surface lands in the message's own buffer with nothing owned on the way.
    pub fn display_name(self, registries: &RunRegistries) -> TypeNameDisplay<'_> {
        TypeNameDisplay {
            ktype: self,
            registries,
        }
    }

    /// Whether this type's surface opens with the type sigil `:` — the predicate a parameter
    /// position consults to decide whether to prefix one of its own, without inspecting rendered
    /// text. True for exactly the compound arms [`write_name`](Self::write_name) opens with `:(`
    /// or `:{`, plus a deferred return whose stored expression surface already carries it.
    pub fn surface_opens_sigil(self, registries: &RunRegistries) -> bool {
        registries.types.with_node(self, |node| match node {
            TypeNode::List { .. }
            | TypeNode::Dict { .. }
            | TypeNode::Record { .. }
            | TypeNode::KFunction { .. }
            | TypeNode::Union { .. }
            | TypeNode::ConstructorApply { .. } => true,
            TypeNode::DeferredReturn(surface) => surface.opens_sigil(),
            _ => false,
        })
    }

    /// Surface-syntax rendering as an owned `String` — [`display_name`](Self::display_name) for a
    /// caller that keeps the text rather than writing it somewhere.
    pub fn name(self, registries: &RunRegistries) -> String {
        self.display_name(registries).to_string()
    }

    /// Stable entry point for diagnostic rendering. Reserved seam for cycle-aware printing.
    pub fn render(self, registries: &RunRegistries) -> String {
        self.name(registries)
    }

    /// The bare Type token this type names itself by, as the classified symbol — or `None` for
    /// a type whose [`name`](Self::name) is compound surface syntax rather than one token.
    ///
    /// No text is hashed on any arm: a node that carries its declared name answers the symbol
    /// stored in it, and a builtin leaf answers its fixed spelling's [`StaticName`] memo (the
    /// same statics [`name`](Self::name) renders from, so the two doors agree by construction).
    /// Static spellings are recorded in the run's label interner here, matching what declaring
    /// the name from text would have recorded, so rendering resolves either way.
    pub fn name_symbol(self, registries: &RunRegistries) -> Option<TypeSymbol> {
        let fixed = |name: &StaticName<TypeSymbol>| Some(registries.labels.record(name));
        // Read in place: every arm answers a stored symbol or a static memo, so nothing here
        // needs the node to outlive the read, and a `Signature` node's schema is never copied.
        registries.types.with_node(self, |node| match node {
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
            TypeNode::OfKind(kind) => Some(kind.surface_symbol(&registries.labels)),
            TypeNode::AbstractType { name, .. } => Some(*name),
            TypeNode::SetMember { name, .. } => Some(*name),
            TypeNode::Signature { schema_digest, .. } => (*schema_digest == empty_schema_digest())
                .then(|| registries.labels.record(&MODULE_NAME)),
            TypeNode::List { .. }
            | TypeNode::Dict { .. }
            | TypeNode::Record { .. }
            | TypeNode::KFunction { .. }
            | TypeNode::DeferredReturn(_)
            | TypeNode::Union { .. }
            | TypeNode::ConstructorApply { .. }
            | TypeNode::Sibling(_) => None,
        })
    }

    /// Classify a *type* into its shallow dispatch [`KKind`] — the value-side direction of
    /// `OfKind`. A signature is `Signature`, a user-declared nominal is its family (`NewType` /
    /// `TypeConstructor`, read off its member node), an abstract member is its declared order,
    /// and every other type is `ProperType`. Never returns `KKind::AnyType` (a slot-only
    /// expectation). Applied to the type a type value carries — or a runtime value's `ktype()` —
    /// to match it against an `OfKind` slot.
    pub fn kind_of(self, types: &TypeRegistry) -> KKind {
        types.with_node(self, |node| match node {
            TypeNode::Signature { .. } => KKind::Signature,
            // A nominal carries its family on its member node; a `ConstructorApply` defers to its
            // constructor (a `TypeConstructor`-kind member, or an abstract constructor).
            TypeNode::SetMember { kind, .. } => *kind,
            TypeNode::ConstructorApply { constructor, .. } => constructor.kind_of(types),
            // An abstract member with declared parameters is a constructor; without them it is a
            // proper type.
            TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => {
                KKind::TypeConstructor
            }
            // A union is a proper type value — it classifies against `OfKind(ProperType)` slots
            // and never against a nominal-family kind.
            _ => KKind::ProperType,
        })
    }
}

/// Write a record's fields as the comma-free `name :type` group the `:{…}` surface re-parses —
/// a record type's own body, and the parameter list of an `:(FN :{…} -> _)`.
/// A leaf type surface gets a `:` prefix; one that already opens a sigil
/// (`:(LIST OF Number)`) is left as-is (no `::`), decided by
/// [`KType::surface_opens_sigil`] rather than by looking at text already written.
fn write_param_record(
    f: &mut std::fmt::Formatter<'_>,
    params: &Record<KType>,
    registries: &RunRegistries,
) -> std::fmt::Result {
    for (index, (key, kt)) in params.iter().enumerate() {
        if index > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{} ", display_label(key.symbol(), registries))?;
        if !kt.surface_opens_sigil(registries) {
            f.write_str(":")?;
        }
        kt.write_name(f, registries)?;
    }
    Ok(())
}

/// A label's text, resolved through the run's interner. Every syntactic label is interned where it
/// is built, so a miss means the symbol came from somewhere else entirely — a runtime probe, a
/// hand-built handle. Rendering stays total: the placeholder prints instead of panicking, because
/// error formatting must never be the thing that fails.
pub fn render_label(symbol: Symbol, registries: &RunRegistries) -> String {
    registries.labels.render(symbol)
}

/// [`render_label`] as a `Display` view — what a `format!` argument naming a label uses, so the
/// text lands in the message's buffer without a `String` of its own on the way.
pub fn display_label(
    symbol: Symbol,
    registries: &RunRegistries,
) -> crate::machine::model::labels::LabelDisplay<'_> {
    registries.labels.display(symbol)
}

/// The structural rendering of a non-empty interface: `SIG (member: Type, …)` over every member
/// the schema names — abstract, manifest and value slot alike — in member-name order, which is
/// the only order the schema's unordered maps admit deterministically.
fn write_sig_schema(
    f: &mut std::fmt::Formatter<'_>,
    schema: &SigSchema,
    registries: &RunRegistries,
) -> std::fmt::Result {
    // Presentation order is alphabetical by member *text*, compared in the interner rather than
    // rendered first, so the staged run holds symbols. The digest feeds sort by symbol instead —
    // identity needs a canonical order, not a readable one. Eight members inline covers every
    // interface the tree declares; a wider one spills to the heap for the length of the write.
    let mut members: SmallVec<[(Symbol, KType); 8]> = schema
        .abstract_members
        .iter()
        .chain(schema.manifest_members.iter())
        .map(|(name, kt)| (name.symbol(), *kt))
        .chain(
            schema
                .value_slots
                .iter()
                .map(|(name, kt)| (name.symbol(), *kt)),
        )
        .collect();
    members.sort_by(|a, b| registries.labels.compare_texts(a.0, b.0));
    f.write_str("SIG (")?;
    for (index, (name, kt)) in members.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{}: ", display_label(*name, registries))?;
        kt.write_name(f, registries)?;
    }
    // Keyworded members follow the named ones, each as the head shape declaring it. They are keyed
    // by a call shape rather than a name, so they sort by the schema's canonical key order rather
    // than by member text.
    let mut written = members.len();
    for (key, overloads) in sorted_keyworded(schema) {
        for overload in overloads {
            if written > 0 {
                f.write_str(", ")?;
            }
            f.write_str(&render_keyworded_head(key, *overload, registries))?;
            written += 1;
        }
    }
    f.write_str(")")
}

/// A [`KType::display_name`] view: one type handle plus the registries its content lives in.
pub struct TypeNameDisplay<'r> {
    ktype: KType,
    registries: &'r RunRegistries,
}

impl std::fmt::Display for TypeNameDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.ktype.write_name(f, self.registries)
    }
}

/// A handle prints as its digest and nothing else: rendering content would need a registry, which
/// a `Formatter`-only signature cannot reach, and the digest is the whole identity anyway.
impl std::fmt::Debug for KType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KType(0x{:032x})", self.0.0)
    }
}

#[cfg(test)]
mod tests;
