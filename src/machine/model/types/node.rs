//! [`TypeNode`] — one interned type's content, the thing a [`KType`] handle names.
//!
//! A node stores its variant tag, its scalar payload (names, [`ScopeId`]s, a signature's schema
//! shape), and **handles to its child types** — never owned substructure. Nodes are immutable
//! from the moment they are interned, and the registry that owns them is insert-only for the
//! life of a run, so a handle stays dereferenceable as long as its registry lives.
//!
//! Interning and node reads live on [`TypeRegistry`](super::registry::TypeRegistry); the digest
//! recipe per variant lives in [`type_digest`](super::type_digest).
//!
//! See [design/typing/type-registry.md](../../../../design/typing/type-registry.md).

use crate::machine::core::ScopeId;
use crate::machine::model::labels::TypeSymbol;

use super::kkind::KKind;
use super::ktype::KType;
use super::record::Record;
use super::sig_schema::{SigSchema, TypeMemberMap};
use super::signature::{DeferredReturnSurface, DispatchTokenElement};
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
    /// Binder-position slot: captures a bare name token of either class raw
    /// (`ExpressionPart::Identifier` or `::Type`), delivered as `Held::Name`. Never resolves.
    NameToken,
    /// Binder-position slot for a Type-class name only: captures a bare `ExpressionPart::Type`
    /// raw, delivered as `Held::Name(BinderSymbol::Type(_))`. Never resolves — unlike
    /// `OfKind(ProperType)`, which is a type *reference* slot and lowers builtin names.
    TypeNameToken,
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
    /// The uninhabited bottom of the type lattice: admitted by no value, more specific than every
    /// other type, and the identity element of both join and union canonicalization. Produced by
    /// [`TypeRegistry::meet`](super::registry::TypeRegistry::meet) and by the empty container's
    /// element join; no value's `ktype()` is ever `Never`. Spellable as the builtin name `Never`,
    /// where it declares a slot nothing fills.
    Never,
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
    /// `name` and each parameter name are Type-class labels, interned at their declaration; a
    /// diagnostic naming one resolves the text through the run's label interner.
    ///
    /// All four fields are identity; nothing here is digest-excluded. `param_names` feeds kind
    /// classification and `source` feeds member substitution, so both are functional reads and
    /// interning must not collapse across them.
    AbstractType {
        source: ScopeId,
        name: TypeSymbol,
        param_names: Vec<TypeSymbol>,
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
    /// Structural record type (`:{x :Number, y :Str}`) — a `BinderSymbol`-keyed field schema
    /// (a field name may be an identifier or a capitalized `Type` token) with width/depth
    /// subtyping, order-blind by `(name, type)` for identity and declaration-ordered for
    /// rendering.
    Record {
        fields: Record<KType>,
    },
    /// A function type `(params) -> ret`. koan has no positional call syntax, so a
    /// function-typed slot records the names a caller must use to invoke what it receives.
    KFunction {
        params: Record<KType>,
        ret: KType,
    },
    /// An **expression shape** — the type of a keyworded, positional definition reached by
    /// dispatch: the interleaved element sequence a call must spell (fixed keywords and typed
    /// argument positions, in order), the type parameters the shape binds ahead of it, and the
    /// return type.
    ///
    /// Distinct from [`Self::KFunction`] as a matter of representation, not of encoding: a lambda
    /// takes a record of named arguments and is reached by name, a shape is reached by its
    /// keyword/argument sequence and its argument *positions* are load-bearing, which a
    /// canonically ordered params record erases. So no shape is ever equal to, satisfies, or is
    /// satisfied by a lambda type.
    ///
    /// Argument **names** are binder-side only and are absent here: two definitions differing
    /// only in parameter names project one shape. Quantifier names are the exception — the later
    /// elements and the return dereference them through [`Self::Quantified`] — but they are a
    /// *binding*, not identity: the digest feeds the count alone, so alpha-variants intern once
    /// and `quantifiers` holds whichever spelling was interned first.
    ExpressionShape {
        /// The type parameters this shape binds, in `Quantified(index)` order. Render-only:
        /// the arity is identity, the names are not.
        quantifiers: Vec<TypeSymbol>,
        /// The call shape: at least one [`DispatchTokenElement::Keyword`], interleaved with the
        /// argument positions' declared types.
        elements: Box<[DispatchTokenElement]>,
        ret: KType,
    },
    /// The `index`-th quantifier of the enclosing [`Self::ExpressionShape`] — the unconstrained
    /// top in every type relation, and solved per call by the argument-validation unifier. A leaf
    /// rather than a name, so two shapes alpha-equivalent under a renaming of their quantified
    /// parameters intern to one node.
    Quantified(usize),
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
    /// of `schema`, computed once at construction. `WITH` specialization folds its pins into the
    /// schema before interning ([`SigSchema::fold_pins`]), so the node stores no pin set — a
    /// specialized interface and the equivalent concrete declaration are one content.
    Signature {
        schema: SigSchema,
        schema_digest: TypeDigest,
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
    /// digest plus its index in that component's canonical order — the numeric order of the
    /// members' name symbols — so two independently built components with the same content intern
    /// to the same nodes.
    ///
    /// `name` is a Type-class label interned at the declaration that minted the member; a
    /// diagnostic naming one resolves the text through the run's label interner.
    ///
    /// `scc_size`, `name`, `kind`, and `schema` are excluded from the digest because they are
    /// exactly the inputs `scc_digest` was computed over — a handle determines them. The member
    /// records no origin of its own, so interning may collapse digest-equal groups freely.
    SetMember {
        scc_digest: TypeDigest,
        index: usize,
        scc_size: usize,
        name: TypeSymbol,
        kind: KKind,
        schema: NodeSchema,
    },
}

/// A sealed member's schema, over absolute member handles: every sibling reference inside it is
/// the sibling's own [`KType`], which is what makes a group's composition edges cyclic. The
/// pre-seal window carries the relative twin of this shape.
#[derive(Clone)]
pub enum NodeSchema {
    /// Fresh nominal over a transparent representation.
    NewType(KType),
    /// Higher-kinded constructor: erased-parameter variant schema plus parameter names. Both the
    /// schema's keys and the parameter names are Type-class labels, interned at the declaration
    /// that mints the family, so the schema compares and clones without touching text.
    TypeConstructor {
        schema: TypeMemberMap,
        param_names: Vec<TypeSymbol>,
    },
}
