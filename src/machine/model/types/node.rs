//! [`TypeNode`] — one interned type's content, the thing a [`KType`] handle names.
//!
//! A node stores its variant tag, its scalar payload (names, [`ScopeId`]s, a signature's schema
//! shape), and **handles to its child types** — never owned substructure. Nodes are immutable
//! from the moment they are interned, and the registry that owns them is insert-only for the
//! life of a run, so a handle stays dereferenceable as long as its registry lives.
//!
//! Interning and node reads live on [`TypeRegistry`](super::registry::TypeRegistry); the digest
//! recipe per variant lives in [`type_digest`](super::type_digest), which is the one place the
//! hash function is touched.
//!
//! See [design/typing/type-registry.md](../../../../design/typing/type-registry.md).

use std::collections::HashMap;

use crate::machine::core::ScopeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::record::Record;
use super::sig_schema::SigSchema;
use super::signature::DeferredReturnSurface;
use super::type_digest::TypeDigest;

/// The content of one interned type. Every child position is a [`KType`] handle, so a node is
/// shallow: cloning one out of the registry copies its scalar payload and its children's
/// digests, never a type subtree.
#[derive(Clone)]
pub enum TypeNode {
    Number,
    Str,
    Bool,
    Null,
    Identifier,
    /// Lazy slot: accepts an unevaluated `ExpressionPart::Expression`, so the builtin chooses
    /// when (or whether) to run it.
    KExpression,
    /// Lazy slot for a `:(...)` type expression — captured raw so a builtin can defer a
    /// param-referencing dotted/sigil return to per-call elaboration.
    SigiledTypeExpr,
    /// Lazy slot for a `:{…}` record type — captured raw so the NEWTYPE record-repr declarator
    /// owns its elaboration and threads its own binder name.
    RecordType,
    Any,
    /// Type-accepting argument slot, carrying the shallow [`KKind`] it admits — and the type a
    /// non-signature type value reports (`OfKind(ProperType)`).
    OfKind(KKind),
    /// Abstract type member named by a SIG slot or minted by opaque ascription.
    ///
    /// `source` is the binder the member is named against. `nonce` is the generativity
    /// mechanism: `None` for a SIG-body declaration, `Some(<per-application module scope id>)`
    /// for the mint `:|` produces, so two opaque ascriptions of one SIG never unify.
    /// `param_names` carries the member's order — empty is a first-order proper type
    /// (`TYPE Elt`), non-empty a constructor over those named parameters (`TYPE (Elem AS Wrap)`).
    ///
    /// All four fields are identity; nothing here is digest-excluded. `param_names` feeds kind
    /// classification and `source` feeds member substitution, so both are functional reads and
    /// interning must not collapse across them.
    AbstractType {
        source: ScopeId,
        name: String,
        param_names: Vec<String>,
        nonce: Option<ScopeId>,
    },
    /// `List<element>`. Bare `List` lowers to `List<Any>`.
    List {
        element: KType,
    },
    /// `Dict<key, value>`. Bare `Dict` lowers to `Dict<Any, Any>`.
    Dict {
        key: KType,
        value: KType,
    },
    /// Structural record type (`:{x :Number, y :Str}`) — an identifier-keyed field schema with
    /// width/depth subtyping, order-blind by `(name, type)` for identity and declaration-ordered
    /// for rendering.
    Record {
        fields: Record<KType>,
    },
    /// A function type `(params) -> ret`. koan has no positional call syntax, so a
    /// function-typed slot records the names a caller must use to invoke what it receives.
    KFunction {
        params: Record<KType>,
        ret: KType,
    },
    /// Untagged structural disjunction — the type `:(A | B)`. Members are canonical:
    /// deduplicated, no nested `Union`, always two or more. Identity is order-blind.
    /// Build through [`TypeRegistry::union_of`](super::registry::TypeRegistry::union_of), the
    /// single canonicalizing entry point.
    Union {
        members: Vec<KType>,
    },
    /// Application of a higher-kinded type constructor to argument types. `arguments` maps each
    /// of the constructor's parameter names to the elaborated argument type; the digest feeds
    /// them name-sorted, so the same name-to-type map is the same application however written.
    ConstructorApply {
        constructor: KType,
        arguments: Record<KType>,
    },
    /// A module signature — owned interface content. A `SIG`-declared interface, a module's
    /// self-sig, and the empty signature (the lattice top `:Module` lowers to) are all this one
    /// node, distinguished only by `schema`.
    ///
    /// The node carries no binder and no label: two textually identical SIG declarations are one
    /// type. `schema_digest` is [`schema_content_digest`](super::type_digest::schema_content_digest)
    /// of `schema`, computed once at construction; `pinned_slots` carries `WITH` abstract-type
    /// specializations, order-preserving so identity is deterministic.
    Signature {
        schema: SigSchema,
        schema_digest: TypeDigest,
        pinned_slots: Vec<(String, KType)>,
    },
    /// Confined carrier for a synthesized FN `ret` slot whose source return is deferred — a
    /// per-call-elaborated return like `-> er` or `-> er.Carrier`. Holds only the hashable
    /// surface shadow, and admits nothing on its own.
    DeferredReturn(DeferredReturnSurface),
    /// A relative sibling reference inside a pre-seal recursive-group window: the sibling's bare
    /// index, meaningful only against the ambient window. Ordinary registry content — immutable
    /// and content-addressed like any other node — but it never appears in a sealed schema,
    /// never reaches the predicates, and never rides a value.
    Sibling(usize),
    /// One sealed member of a recursive group. Identity is its strongly-connected component's
    /// digest plus its index in that component's canonical (name) order, so two independently
    /// built components with the same content intern to the same nodes.
    ///
    /// `scc_size`, `name`, `kind`, and `schema` are excluded from the digest because they are
    /// exactly the inputs `scc_digest` was computed over — a handle determines them. The member
    /// records no origin of its own, so interning may collapse digest-equal groups freely.
    SetMember {
        scc_digest: TypeDigest,
        index: usize,
        scc_size: usize,
        name: String,
        kind: KKind,
        schema: NodeSchema,
    },
    /// First-class handle to a whole declared group, bound by a `RECURSIVE TYPES` group name.
    /// Members are the group's declared members in declaration order — a group may span several
    /// components, so this is a declaration boundary rather than an identity unit. Inert in
    /// value dispatch: it names a group of types, not a value type.
    Group {
        members: Vec<KType>,
    },
}

/// A sealed member's schema, over absolute member handles: every sibling reference inside it is
/// the sibling's own [`KType`], which is what makes a group's composition edges cyclic. The
/// pre-seal window carries the relative twin of this shape.
#[derive(Clone)]
pub enum NodeSchema {
    /// Fresh nominal over a transparent representation.
    NewType(KType),
    /// Higher-kinded constructor: erased-parameter variant schema plus parameter names.
    TypeConstructor {
        schema: HashMap<String, KType>,
        param_names: Vec<String>,
    },
}
