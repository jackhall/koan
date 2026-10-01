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
//! See [README.md](README.md) § The node vocabulary.

use crate::memory::ScopeId;
use crate::symbols::{BinderSymbol, TypeSymbol};

use super::digest::TypeDigest;
use super::handle::KType;
use super::kind::KKind;
use super::record::Record;
use super::schema::SigSchema;
use super::shape::{DeferredReturnSurface, DispatchTokenElement};
use super::unify::Interval;

/// The content of one interned type. Every child position is a [`KType`] handle and every run is a
/// `'run` slice, so reading a node out of the registry copies its scalar payload and a few fat
/// pointers, never a type subtree.
#[derive(Clone, Copy)]
pub enum TypeNode<'run> {
    Number,
    Str,
    Bool,
    Null,
    /// A lone value name of code (`#(y)`), under [`Self::Name`].
    Identifier,
    /// A lone token of code — spelled `Symbol`: a [`Self::Name`] or a [`Self::Keyword`].
    Symbol,
    /// A lone type name of code (`#(Carrier)`), under [`Self::Name`]. Never resolves — unlike
    /// `OfKind(ProperType)`, which is a type *reference* slot and lowers builtin names.
    TypeNameToken,
    /// One statement of code — spelled `Expression`: a lone part or a run of parts.
    Expression,
    /// A lone `:(…)` type expression of code, under [`Self::Expression`].
    SigiledTypeExpr,
    /// A lone `:{…}` record type of code, under [`Self::Expression`].
    RecordType,
    /// A lone scalar literal or nested quote of code (`#(42)`, `#(#(x))`), under
    /// [`Self::Expression`].
    Literal,
    /// Statements of code — the kind every body slot takes; written, it is two or more statements.
    /// The one code kind directly under [`Self::AnyCode`].
    Block,
    /// One statement that declares a name or a shape: a `VAL`, a bodyless head, or a
    /// [`Self::Binder`].
    Declaration,
    /// One statement that declares and installs where it is written, as `LET x = 1` does.
    Binder,
    /// A lone value or type name of code — what a declaration binds.
    Name,
    /// A lone keyword of code, as an operator's symbol (`#(+)`).
    Keyword,
    /// The lattice top: above the three family tops, and the default bound of a rigid variable.
    Any,
    /// The value family's top, spelled `Value`: above every type whose values are ordinary values.
    AnyValue,
    /// The code family's top, spelled `Code`: above every code kind.
    AnyCode,
    /// A code kind and the names its code needs where it is built, spelled
    /// `:(Expression NEEDING #[y])`: the carried type of a quote whose `\` marks no binder in its
    /// own code fills. Below `Code`, above the same kind needing more, and below the same kind
    /// needing fewer; the bare kind is the kind needing nothing, so `names` is never empty. Build
    /// through [`TypeRegistry::code_needing`](super::registry::TypeRegistry::code_needing).
    CodeNeeding {
        /// A code kind below `Code`.
        kind: KType,
        /// Symbol-sorted and deduplicated: the needed names are a set.
        names: &'run [BinderSymbol],
    },
    /// The uninhabited bottom: admitted by no value, below every other type, and the identity
    /// element of both [`join`](super::lattice::join) and union canonicalization. Spellable as
    /// the builtin name `Never`, where it declares a slot nothing fills.
    Never,
    /// Type-accepting argument slot, carrying the shallow [`KKind`] it admits — and the type a
    /// non-signature type value reports (`OfKind(ProperType)`).
    OfKind(KKind),
    /// A **named rigid variable**: a signature's head parameter (no `nonce`), or the carrier an
    /// opaque `:|` view mints for one (a `nonce` nothing else can name, so two opaque ascriptions
    /// of one signature never unify). `bound` is what bounds it, [`KType::ANY`] unless declared.
    ///
    /// Named where [`Self::Quantified`] is positional: a parameter is substituted by name within
    /// its own signature, and `WITH` pins it by name. The three rigid variables — this,
    /// [`Self::Quantified`] and [`Self::Lexical`] — share the rigid rule in the order and the role
    /// of the rigid side in a specificity check.
    ///
    /// Every field is identity.
    Parameter {
        name: TypeSymbol,
        bound: KType,
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
    /// Structural record type (`:{x :Number, y :Str}`) — a [`Record`] field schema with
    /// width/depth subtyping, order-blind by `(name, type)` for identity and declaration-ordered
    /// for rendering.
    Record {
        fields: Record<'run>,
    },
    /// A function type `(params) -> ret`. koan has no positional call syntax, so a
    /// function-typed slot records the names a caller must use to invoke what it receives.
    ///
    /// A non-empty `quantifiers` makes it a **binder** of its own, exactly as an
    /// [`Self::ExpressionShape`] is: the group keeps every variable it declares, its names are
    /// render-only, and the digest feeds the arity and the bounds. An empty one binds nothing and is
    /// transparent — an unquantified
    /// function type written inside a quantified head keeps reading that head's variables.
    KFunction {
        /// The type parameters this function binds, in `Quantified` index order. Render-only, as
        /// a shape's are.
        quantifiers: &'run [TypeSymbol],
        /// Each quantifier's bound, in the same order.
        bounds: &'run [KType],
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
    /// render-only too: the digest feeds the arity and the bounds, so alpha-variants intern once and
    /// `quantifiers` holds whichever spelling was interned first. A variable no position names
    /// carries its bound in `bounds` alone.
    ExpressionShape {
        /// The type parameters this shape binds, in `Quantified` index order. Render-only:
        /// the arity is identity, the names are not.
        quantifiers: &'run [TypeSymbol],
        /// Each quantifier's bound, in the same order.
        bounds: &'run [KType],
        /// The call shape: fixed keywords interleaved with the argument positions' declared types.
        elements: &'run [DispatchTokenElement],
        /// Each slot's priority class, in slot order, dense from 0 — the ranking a dispatch admits
        /// and ranks the slots by, class by class. Empty for written order, the canonical spelling
        /// of `0..n`, so an unranked shape stores and digests nothing for it.
        classes: &'run [u8],
        ret: KType,
    },
    /// A **rigid variable bound by the enclosing binder** — a [`Self::ExpressionShape`], or a
    /// [`Self::KFunction`] carrying a group: the `index`-th member of that group, bounded by
    /// `bound`. Positional rather than named, so two binders alpha-equivalent under a renaming of
    /// their parameters intern to one node.
    Quantified {
        index: usize,
        bound: KType,
    },
    /// A **lexical variable**: a name only a run binds — a `FOR ALL` or `:Type` parameter, a name a
    /// `USING` surfaces, a quote's hole, a type binder the load left unknown — read where the
    /// program loads. Positional by its `level` along the lexical chain of bodies that declares
    /// it, lying between `lower` and `bound`, and named `name`. `lower` is `Never` for every name
    /// the elaborator reads; a class-by-class walk over static types mints one with a higher lower
    /// end ([`ranking`](super::ranking)). No binder captures it and no solve binds it: a run
    /// replaces it with the type bound at its level
    /// ([`substitute_levels`](super::substitute::substitute_levels)).
    ///
    /// Every field is identity: along one chain a level is one name, and the name renders it.
    Lexical {
        level: usize,
        name: TypeSymbol,
        lower: KType,
        bound: KType,
    },
    /// Untagged structural disjunction — the type `:(A | B)`. Members are canonical:
    /// deduplicated, no nested `Union`, no member below the rest, always two or more, in the order
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
    /// A module signature — owned interface content: a `SIG`-declared interface, a module's
    /// self-signature, a view's signature, or the empty signature (the lattice top `:Module` lowers
    /// to), told apart by `schema`.
    ///
    /// The node carries no binder and no label: two textually identical SIG declarations are one
    /// type. `schema_digest` is
    /// [`schema_content_digest`](super::digest::schema_content_digest) of `schema`, computed once
    /// at construction.
    Signature {
        schema: SigSchema<'run>,
        schema_digest: TypeDigest,
    },
    /// An **application** of a declared signature: `signature` with some of its head parameters
    /// pinned, spelled `Stack WITH {Elt = Number}`. `signature` is a [`Self::Signature`] of origin
    /// `Declared`; `pins` is non-empty and keyed by the parameters' names. Build through
    /// [`TypeRegistry::signature_apply`](super::registry::TypeRegistry::signature_apply), which
    /// answers `signature` itself for no pins.
    SignatureApply {
        signature: KType,
        pins: Record<'run>,
    },
    /// A **set of applications**, two or more, none lying above another, sorted by handle so the
    /// set's identity is order-blind: what the meet of two signature types is. Each member is a
    /// [`Self::Signature`] or a [`Self::SignatureApply`], never the empty signature and never a
    /// meet. Build through
    /// [`TypeRegistry::signature_meet`](super::registry::TypeRegistry::signature_meet).
    SignatureMeet {
        members: &'run [KType],
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

impl TypeNode<'_> {
    /// Whether this node binds a quantifier group of its own: a shape always — its group may be
    /// empty — and a function only when it carries one. This is what every walk asks before
    /// stepping into a child, so a `Quantified` under it reads against the right group.
    pub fn binds_quantifiers(&self) -> bool {
        match self {
            TypeNode::ExpressionShape { .. } => true,
            TypeNode::KFunction { quantifiers, .. } => !quantifiers.is_empty(),
            _ => false,
        }
    }

    /// A rigid variable's bound — a [`Self::Quantified`]'s, a [`Self::Lexical`]'s or a
    /// [`Self::Parameter`]'s — or `None` for any other node.
    pub fn rigid_bound(&self) -> Option<KType> {
        match self {
            TypeNode::Quantified { bound, .. }
            | TypeNode::Lexical { bound, .. }
            | TypeNode::Parameter { bound, .. } => Some(*bound),
            _ => None,
        }
    }

    /// A rigid variable's lower end: a [`Self::Lexical`]'s own, `Never` for the other two, `None`
    /// for any other node.
    pub fn rigid_lower(&self) -> Option<KType> {
        match self {
            TypeNode::Lexical { lower, .. } => Some(*lower),
            TypeNode::Quantified { .. } | TypeNode::Parameter { .. } => Some(KType::NEVER),
            _ => None,
        }
    }

    /// A rigid variable's two ends as an interval, or `None` for any other node.
    pub fn rigid_interval(&self) -> Option<Interval> {
        Some(Interval {
            lower: self.rigid_lower()?,
            upper: self.rigid_bound()?,
        })
    }
}

/// A sealed member's schema, over absolute member handles: every sibling reference inside it is
/// the sibling's own [`KType`], which is what makes a group's composition edges cyclic. The
/// pre-seal window carries the relative twin of this shape.
#[derive(Clone, Copy)]
pub enum NodeSchema<'run> {
    /// Fresh nominal over a transparent representation.
    NewType(KType),
    /// Higher-kinded constructor: the representation a construction through the family wraps,
    /// written over `Quantified { index, bound: Any }` for `param_names[index]`, or `None` for a
    /// family that constructs nothing; plus its parameter names, Type-class symbols interned at
    /// the declaration, stored symbol-sorted because a constructor's identity is their set.
    TypeConstructor {
        representation: Option<KType>,
        param_names: &'run [TypeSymbol],
    },
}
