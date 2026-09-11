//! [`TypeNode`] — one interned type's content, the thing a [`KType`] handle names.
//!
//! A node stores its variant tag, its scalar payload (names, [`ScopeId`]s, a signature's schema
//! shape), and **handles to its child types** — never owned substructure. Every run a node holds —
//! a union's members, a shape's elements, a record's fields, a schema's tables — is a slice in the
//! run region, so a node is `Copy` and carries no drop glue. Nodes are immutable from the moment
//! they are interned, and the registry that owns them is insert-only for the life of a run, so a
//! handle stays dereferenceable as long as its registry lives.
//!
//! Interning and node reads live on [`TypeRegistry`](super::registry::TypeRegistry); the digest
//! recipe per variant lives in [`digest`](super::digest).
//!
//! See [design/typing/type-lattice.md](../../design/typing/type-lattice.md).

use crate::memory::ScopeId;
use crate::parse::TypeSymbol;

use super::digest::TypeDigest;
use super::handle::KType;
use super::kind::KKind;
use super::record::Record;
use super::schema::{Members, SigSchema};
use super::shape::{DeferredReturnSurface, DispatchTokenElement};

/// The content of one interned type. Every child position is a [`KType`] handle and every run is a
/// `'run` slice, so reading a node out of the registry copies its scalar payload and a few fat
/// pointers, never a type subtree.
#[derive(Clone, Copy)]
pub enum TypeNode<'run> {
    Number,
    Str,
    Bool,
    Null,
    Identifier,
    /// Binder-position slot: captures a bare name token of either class raw. Never resolves.
    NameToken,
    /// Binder-position slot for a Type-class name only, captured raw. Never resolves — unlike
    /// `OfKind(ProperType)`, which is a type *reference* slot and lowers builtin names.
    TypeNameToken,
    /// Lazy slot: accepts an unevaluated expression, so the builtin chooses when (or whether) to
    /// run it.
    KExpression,
    /// Lazy slot for a `:(...)` type expression — captured raw so a builtin can defer a
    /// param-referencing dotted/sigil return to per-call elaboration.
    SigiledTypeExpr,
    /// Lazy slot for a `:{…}` record type — captured raw so the NEWTYPE record-repr declarator
    /// owns its elaboration and threads its own binder name.
    RecordType,
    /// The lattice top: above every type, and the default bound of a rigid variable.
    Any,
    /// The uninhabited bottom: admitted by no value, below every other type, and the identity
    /// element of both [`join`](super::lattice::join) and union canonicalization. Spellable as
    /// the builtin name `Never`, where it declares a slot nothing fills.
    Never,
    /// Type-accepting argument slot, carrying the shallow [`KKind`] it admits — and the type a
    /// non-signature type value reports (`OfKind(ProperType)`).
    OfKind(KKind),
    /// A **rigid variable named by a signature member**: an abstract type member declared by a
    /// SIG slot or minted by opaque ascription.
    ///
    /// Named and editable where [`Self::Quantified`] is positional and alpha-equivalent, because
    /// members are reached by name and schemas are edited by name. The two share the rigid rule
    /// in the order, the substitution mechanism, and the role of the rigid side in a specificity
    /// check.
    ///
    /// `source` is the binder the member is named against. `nonce` is the generativity
    /// mechanism: `None` for a SIG-body declaration, `Some(<per-application module scope id>)`
    /// for the mint `:|` produces, so two opaque ascriptions of one SIG never unify.
    /// `param_names` carries the member's order — empty is a first-order proper type
    /// (`TYPE Elt`), non-empty a constructor over those named parameters (`TYPE (Elem AS Wrap)`).
    /// They are stored symbol-sorted: a constructor's identity is its parameter-name *set*.
    /// `bound` is what the variable stands over, [`KType::ANY`] unless declared.
    ///
    /// Every field is identity; nothing here is digest-excluded.
    AbstractType {
        source: ScopeId,
        name: TypeSymbol,
        param_names: &'run [TypeSymbol],
        nonce: Option<ScopeId>,
        bound: KType,
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
    /// Structural record type (`:{x :Number, y :Str}`) — a [`Record`] field schema with
    /// width/depth subtyping, order-blind by `(name, type)` for identity and declaration-ordered
    /// for rendering.
    Record {
        fields: Record<'run>,
    },
    /// A function type `(params) -> ret`. koan has no positional call syntax, so a
    /// function-typed slot records the names a caller must use to invoke what it receives.
    KFunction {
        params: Record<'run>,
        ret: KType,
    },
    /// An **expression shape** — the type of a keyworded, positional definition reached by
    /// dispatch: the interleaved element sequence a call must spell, the type parameters the
    /// shape binds ahead of it, and the return type.
    ///
    /// Distinct from [`Self::KFunction`] as a matter of representation: a lambda takes a record of
    /// named arguments and is reached by name, a shape is reached by its keyword/argument sequence
    /// and its argument *positions* are load-bearing, which a canonically ordered params record
    /// erases. So no shape is ever equal to, satisfies, or is satisfied by a lambda type.
    ///
    /// Argument **names** are binder-side only and are absent here. Quantifier names are
    /// render-only too: the digest feeds the arity, so alpha-variants intern once and
    /// `quantifiers` holds whichever spelling was interned first. Each surviving variable's
    /// *bound* rides on its own [`Self::Quantified`] occurrences, which the canonical form
    /// guarantees exist; `bounds` is the same list read off them once, at intern.
    ExpressionShape {
        /// The type parameters this shape binds, in `Quantified` index order. Render-only:
        /// the arity is identity, the names are not.
        quantifiers: &'run [TypeSymbol],
        /// Each quantifier's bound, in the same order. Digest-excluded: the occurrences already
        /// carry it.
        bounds: &'run [KType],
        /// The call shape: fixed keywords interleaved with the argument positions' declared types.
        elements: &'run [DispatchTokenElement],
        ret: KType,
    },
    /// A **rigid variable bound by the enclosing [`Self::ExpressionShape`]**: the `index`-th
    /// member of its quantifier group, standing over `bound`. Positional rather than named, so
    /// two shapes alpha-equivalent under a renaming of their parameters intern to one node.
    Quantified {
        index: usize,
        bound: KType,
    },
    /// Untagged structural disjunction — the type `:(A | B)`. Members are canonical:
    /// deduplicated, no nested `Union`, no member below another, always two or more, in the order
    /// first written. Identity is order-blind. Build through
    /// [`TypeRegistry::union_of`](super::registry::TypeRegistry::union_of).
    Union {
        members: &'run [KType],
    },
    /// Application of a higher-kinded type constructor to argument types. `arguments` maps each
    /// of the constructor's parameter names to the elaborated argument type; the digest feeds
    /// them name-sorted, so the same name-to-type map is the same application however written.
    ConstructorApply {
        constructor: KType,
        arguments: Record<'run>,
    },
    /// A module signature — owned interface content. A `SIG`-declared interface, a module's
    /// self-sig, and the empty signature (the lattice top `:Module` lowers to) are all this one
    /// node, distinguished only by `schema`.
    ///
    /// The node carries no binder and no label: two textually identical SIG declarations are one
    /// type. `schema_digest` is
    /// [`schema_content_digest`](super::digest::schema_content_digest) of `schema`, computed once
    /// at construction.
    Signature {
        schema: SigSchema<'run>,
        schema_digest: TypeDigest,
    },
    /// Confined carrier for a synthesized FN `ret` slot whose source return is deferred. Holds
    /// only the hashable surface shadow, and admits nothing on its own.
    DeferredReturn(DeferredReturnSurface<'run>),
    /// A relative sibling reference inside a pre-seal recursive-group window: the sibling's bare
    /// index, meaningful only against the ambient window. Ordinary registry content, and the
    /// order relates it as an atom with the same profile as the member it will become — which is
    /// what lets a relative schema's unions canonicalize before their members exist. It never
    /// appears in a sealed schema.
    Sibling(usize),
    /// One sealed member of a recursive group. Identity is its strongly-connected component's
    /// digest plus its index in that component's canonical order — the numeric order of the
    /// members' name symbols — so two independently built components with the same content intern
    /// to the same nodes.
    ///
    /// `scc_size`, `name`, `kind`, and `schema` are excluded from the digest because they are
    /// exactly the inputs `scc_digest` was computed over — a handle determines them.
    SetMember {
        scc_digest: TypeDigest,
        index: usize,
        scc_size: usize,
        name: TypeSymbol,
        kind: KKind,
        schema: NodeSchema<'run>,
    },
}

/// A sealed member's schema, over absolute member handles: every sibling reference inside it is
/// the sibling's own [`KType`], which is what makes a group's composition edges cyclic. The
/// pre-seal window carries the relative twin of this shape.
#[derive(Clone, Copy)]
pub enum NodeSchema<'run> {
    /// Fresh nominal over a transparent representation.
    NewType(KType),
    /// Higher-kinded constructor: erased-parameter variant schema plus parameter names. Both the
    /// schema's keys and the parameter names are Type-class labels, interned at the declaration
    /// that mints the family, and both are stored symbol-sorted — the schema so it is read in one
    /// order, the parameter names because a constructor's identity is their set.
    TypeConstructor {
        schema: Members<'run, TypeSymbol>,
        param_names: &'run [TypeSymbol],
    },
}
