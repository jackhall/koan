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
//! `Dict<Any, Any>` at `from_name` time. There's no bare `KFunction` — "any function" with no
//! signature has nothing to dispatch on, so users write `Function<(args) -> R>` or `Any`.
//!
//! Predicates live in `ktype_predicates.rs`; elaboration lives in `ktype_resolution.rs`.

use super::kkind::KKind;
use super::node::TypeNode;
use super::record::Record;
use super::registry::TypeRegistry;
use super::sig_schema::SigSchema;
use super::type_digest::{empty_schema_digest, TypeDigest};

/// A handle to one interned type: the content digest of its [`TypeNode`], and nothing else.
///
/// Identity is the digest, so two independently built types with the same content are one handle
/// — that is the interning contract, not a coincidence of sharing. `Ord` is the numeric order of
/// the digest: meaningless as a type order, useful only for canonical sorting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KType(TypeDigest);

impl KType {
    // --- Fixed handles ---
    //
    // The nine leaves, the five `OfKind` values, `List<Any>`, `Dict<Any, Any>` and the empty
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
    pub const KEXPRESSION: KType = KType(TypeDigest(0x63c296ef_dbe5d41c_9969ddda_6b0b311c));
    pub const SIGILED_TYPE_EXPR: KType = KType(TypeDigest(0xf6d652dc_848e0f69_4a152496_ddd88b44));
    pub const RECORD_TYPE: KType = KType(TypeDigest(0x387dfced_dc0a5d96_da3b29a5_dde0f32e));
    pub const ANY: KType = KType(TypeDigest(0xd9f70f99_49f95b5c_44d7ce99_10aa1972));

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
    pub const EMPTY_SIGNATURE: KType = KType(TypeDigest(0x1660d74d_20447364_cde2f1b9_3ed245f6));

    /// The type-accepting slot admitting `kind` — one of the five pre-seeded `OfKind` handles.
    pub const fn of_kind(kind: KKind) -> KType {
        match kind {
            KKind::ProperType => KType::PROPER_TYPE,
            KKind::Signature => KType::SIGNATURE_KIND,
            KKind::AnyType => KType::ANY_TYPE,
            KKind::NewType => KType::NEW_TYPE,
            KKind::TypeConstructor => KType::TYPE_CONSTRUCTOR,
        }
    }

    /// Wrap a digest as the handle naming it. Named rather than a public tuple field so the
    /// wrapping is a deliberate act: the only production caller is
    /// [`TypeRegistry::intern`](super::registry::TypeRegistry::intern), which has just computed
    /// the digest of content it is inserting, plus the seal path deriving a member handle from
    /// its component's digest.
    pub(crate) const fn from_digest(digest: TypeDigest) -> KType {
        KType(digest)
    }

    /// This type's content digest — its identity, and its key in the registry's node table.
    pub const fn digest(self) -> TypeDigest {
        self.0
    }

    /// Surface-syntax rendering. The rendered form parses back to the same type through the
    /// dispatch-driven type-language path (see
    /// [type-language via dispatch](../../../../design/typing/type-language-via-dispatch.md)).
    pub fn name(self, types: &TypeRegistry) -> String {
        match types.node(self) {
            TypeNode::Number => "Number".into(),
            TypeNode::Str => "Str".into(),
            TypeNode::Bool => "Bool".into(),
            TypeNode::Null => "Null".into(),
            TypeNode::Identifier => "Identifier".into(),
            TypeNode::KExpression => "KExpression".into(),
            TypeNode::SigiledTypeExpr => "SigiledTypeExpr".into(),
            TypeNode::RecordType => "RecordType".into(),
            TypeNode::Any => "Any".into(),
            TypeNode::OfKind(kind) => kind.surface_keyword().into(),
            TypeNode::List { element } => format!(":(LIST OF {})", element.name(types)),
            TypeNode::Dict { key, value } => {
                format!(":(MAP {} -> {})", key.name(types), value.name(types))
            }
            // `:{x :Number y :Str}` — the braced type-sigil surface. Fields render
            // space-separated like FN params (the field-list parser accepts that).
            TypeNode::Record { fields } => format!(":{{{}}}", render_param_record(&fields, types)),
            TypeNode::KFunction { params, ret } => format!(
                ":(FN ({}) -> {})",
                render_param_record(&params, types),
                ret.name(types)
            ),
            TypeNode::DeferredReturn(surface) => surface.render(),
            // `:(A | B)` — members joined by ` | ` and wrapped in the type sigil. A compound
            // member already opens its own sigil (`:(LIST OF Number)`), which nests fine.
            TypeNode::Union { members } => {
                let rendered: Vec<String> = members.iter().map(|m| m.name(types)).collect();
                format!(":({})", rendered.join(" | "))
            }
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                let bindings: Vec<String> = arguments
                    .iter()
                    .map(|(name, kt)| format!("{name} = {}", kt.name(types)))
                    .collect();
                format!(":({} {{{}}})", constructor.name(types), bindings.join(", "))
            }
            TypeNode::AbstractType { name, .. } => name,
            // A sealed nominal member renders by its own member name — a bare newtype
            // (`:Wrapper`) or a per-variant member reached through its union (`:(Maybe Some)`
            // yields the `Some` member, printed as `Some`).
            TypeNode::SetMember { name, .. } => name,
            TypeNode::Group { members } => {
                let names: Vec<String> = members
                    .iter()
                    .map(|m| match types.node(*m) {
                        TypeNode::SetMember { name, .. } => name,
                        _ => m.name(types),
                    })
                    .collect();
                format!("RECURSIVE TYPES ({})", names.join(" "))
            }
            // A signature names itself by its content: the empty interface is the lattice top
            // `Module`, and any other interface renders its members structurally. There is no
            // declaration label to print — two textually identical `SIG` declarations are one
            // type, so naming either one of them would be a lie about the other.
            TypeNode::Signature {
                schema,
                schema_digest,
            } => {
                if schema_digest == empty_schema_digest() {
                    "Module".to_string()
                } else {
                    render_sig_schema(&schema, types)
                }
            }
            // Diagnostic only: a sibling reference is meaningful against its window and never
            // survives a seal, so nothing outside a mid-seal diagnostic can reach this.
            TypeNode::Sibling(index) => format!("<sibling {index}>"),
        }
    }

    /// Stable entry point for diagnostic rendering. Reserved seam for cycle-aware printing.
    pub fn render(self, types: &TypeRegistry) -> String {
        self.name(types)
    }

    /// Classify a *type* into its shallow dispatch [`KKind`] — the value-side direction of
    /// `OfKind`. A signature is `Signature`, a user-declared nominal is its family (`NewType` /
    /// `TypeConstructor`, read off its member node), an abstract member is its declared order,
    /// and every other type is `ProperType`. Never returns `KKind::AnyType` (a slot-only
    /// expectation). Applied to the type a type value carries — or a runtime value's `ktype()` —
    /// to match it against an `OfKind` slot.
    pub fn kind_of(self, types: &TypeRegistry) -> KKind {
        match types.node(self) {
            TypeNode::Signature { .. } => KKind::Signature,
            // A nominal carries its family on its member node; a `ConstructorApply` defers to its
            // constructor (a `TypeConstructor`-kind member, or an abstract constructor).
            TypeNode::SetMember { kind, .. } => kind,
            TypeNode::ConstructorApply { constructor, .. } => constructor.kind_of(types),
            // An abstract member with declared parameters is a constructor; without them it is a
            // proper type.
            TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => {
                KKind::TypeConstructor
            }
            // A union is a proper type value — it classifies against `OfKind(ProperType)` slots
            // and never against a nominal-family kind.
            _ => KKind::ProperType,
        }
    }
}

/// Render an FN parameter record as the comma-free `name :type` group the `:(FN (...) -> _)`
/// surface re-parses. A leaf type surface gets a `:` prefix; one that already opens a sigil
/// (`:(LIST OF Number)`) is left as-is (no `::`).
fn render_param_record(params: &Record<KType>, types: &TypeRegistry) -> String {
    params
        .iter()
        .map(|(name, kt)| {
            let surface = kt.name(types);
            if surface.starts_with(':') {
                format!("{name} {surface}")
            } else {
                format!("{name} :{surface}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The structural rendering of a non-empty interface: `SIG (member: Type, …)` over every member
/// the schema names — abstract, manifest and value slot alike — in member-name order, which is
/// the only order the schema's unordered maps admit deterministically.
fn render_sig_schema(schema: &SigSchema, types: &TypeRegistry) -> String {
    let mut members: Vec<(&str, KType)> = schema
        .abstract_members
        .iter()
        .chain(schema.manifest_members.iter())
        .chain(schema.value_slots.iter())
        .map(|(name, kt)| (name.as_str(), *kt))
        .collect();
    members.sort_by(|a, b| a.0.cmp(b.0));
    let rendered: Vec<String> = members
        .into_iter()
        .map(|(name, kt)| format!("{name}: {}", kt.name(types)))
        .collect();
    format!("SIG ({})", rendered.join(", "))
}

/// A handle prints as its digest and nothing else: rendering content would need a registry, which
/// a `Formatter`-only signature cannot reach, and the digest is the whole identity anyway.
impl std::fmt::Debug for KType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KType(0x{:032x})", self.0 .0)
    }
}

#[cfg(test)]
mod tests;
