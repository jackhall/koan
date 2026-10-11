//! [`TypeNode`] — one interned type's content, the thing a [`Handle`] names.
//!
//! A node stores its variant tag, its scalar payload (names, [`ContentKey`]s, a signature's schema
//! shape), and **references to its child types' nodes** — never owned substructure. Every run a
//! node holds — a union's members, a shape's elements, a record's fields, a schema's tables — is a
//! slice in the half its node was laid in, so a view is `Copy` and carries no drop glue. Nodes are
//! immutable from the moment they are interned, and every tier is insert-only for its life, so a
//! handle stays dereferenceable in a chain whose tiers hold it.
//!
//! A `TypeNode` is the read view of a node: every field by value, its children read as `H`.
//! Outside the lattice `H` is a handle — read through a [`KType`], a node's children are concrete,
//! and through a [`Parametric`], parametric — while every bound, a lexical variable's lower end, a
//! code kind and an application's signature are always a [`KType`]. Inside the lattice `H` is a
//! reference to the child's node ([`Linked`]), and so is every one of those concrete fields. A
//! tier's table stores a node in its own layout ([`stored`](super::stored)), whose `view_as`
//! is the one conversion. [`Variable`] views the three variable variants as one, and owns the rule
//! for their two ends.
//!
//! Interning and node reads live on [`TypeChain`](super::chain::TypeChain); the digest
//! recipe per variant lives in [`digest`](super::digest).
//!
//! See [README.md](README.md) § The node vocabulary.

use crate::symbols::{BinderSymbol, TypeSymbol};

use super::chain::TypeChain;
use super::component::Component;
use super::digest::TypeDigest;
use super::handle::{Handle, KType, Naming, Parametric, TypeHandle};
use super::kind::KKind;
use super::record::Record;
use super::run::{Elements, Run};
use super::schema::SigSchema;
use super::shape::{Classes, DeferredReturnSurface};
use super::stored::Linked;
use super::tiers::Habitat;
use super::unify::Interval;

/// An opaque identity a carrier is keyed on: what a module layer computes from an opaque view's
/// own content — its operator and signature application over its source module's digest — and
/// the lattice never interprets. Two carriers of one name and met bound under one key are one type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ContentKey(pub u128);

/// A scheme's quantifier group: each variable's name and bound, in group order — the order the
/// scheme door numbered them in. The names are render-only; the arity and the bounds are identity.
/// Laid in the region when its scheme is first interned.
#[derive(Clone, Copy, Debug)]
pub struct Group<'run> {
    pub names: &'run [TypeSymbol],
    pub bounds: &'run [KType],
}

impl Group<'_> {
    /// The group a type that is not a scheme binds: none.
    pub const EMPTY: Group<'static> = Group {
        names: &[],
        bounds: &[],
    };
}

/// The content of one interned type, its children read as `H` and its concrete fields as
/// `H::Bound`. Every run is a `'run` slice under a view, so reading a node out of its tier
/// copies its scalar payload and a few fat pointers, never a type subtree.
#[derive(Clone, Copy)]
pub enum TypeNode<'run, H: Naming<'run> = Handle> {
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
    /// through [`TypeChain::code_needing`](super::chain::TypeChain::code_needing).
    CodeNeeding {
        /// A code kind below `Code`.
        kind: H::Bound,
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
    /// A **named rigid variable**: a signature's head parameter. `bound` is what bounds it,
    /// [`KType::ANY`] unless declared.
    ///
    /// Named where [`Self::Quantified`] is positional: a parameter is substituted by name within
    /// its own signature, and `WITH` pins it by name. The three rigid variables — this,
    /// [`Self::Quantified`] and [`Self::Lexical`] — share the rigid rule in the order and the role
    /// of the rigid side in a specificity check.
    ///
    /// Every field is identity.
    Parameter {
        name: TypeSymbol,
        bound: H::Bound,
    },
    /// The **carrier** an opaque `:|` view hides a head parameter `name` behind, keyed on content
    /// the lattice never interprets (a [`ContentKey`]: two views of equal content share one
    /// carrier, two of different content never unify). A value carries it, so it is concrete, and
    /// the order treats it as an atom: under itself, a union holding it and `Any`; above itself
    /// and `Never`.
    ///
    /// `met` is the bound the view's source met. Outside its view a carrier reveals none, and only
    /// a signature's fit reads it, where a head parameter's bound is checked
    /// ([`Collector::heads`](super::unify::Collector::heads)). It is payload, not a child.
    ///
    /// Every field is identity.
    Carrier {
        name: TypeSymbol,
        key: ContentKey,
        met: H::Bound,
    },
    /// `List<element>`. Bare `List` lowers to `List<Any>`.
    List {
        element: H,
    },
    /// `Dict<key, value>`. Bare `Dict` lowers to `Dict<Any, Any>`.
    Dict {
        key: H,
        value: H,
    },
    /// Structural record type (`:{x :Number, y :Str}`) — a [`Record`] field schema with
    /// width/depth subtyping, order-blind by `(name, type)` for identity and declaration-ordered
    /// for rendering.
    Record {
        fields: Record<'run, H>,
    },
    /// A function type `(params) -> ret`. koan has no positional call syntax, so a
    /// function-typed slot records the names a caller must use to invoke what it receives.
    ///
    /// A function type binds nothing: a `Quantified` inside it reads the enclosing
    /// [`Self::Scheme`]'s group, so a function type written inside a quantified head keeps reading
    /// that head's variables.
    KFunction {
        params: Record<'run, H>,
        ret: H,
    },
    /// An **expression shape** — the type of a keyworded, positional definition reached by
    /// dispatch: the interleaved element sequence a call must spell, and the return type.
    ///
    /// Distinct from [`Self::KFunction`] as a matter of representation: a lambda takes a record of
    /// named arguments and is reached by name, a shape is reached by its keyword/argument sequence
    /// and its argument *positions* are load-bearing, which a canonically ordered params record
    /// erases. So no shape is ever equal to, satisfies, or is satisfied by a lambda type.
    ///
    /// Argument **names** are binder-side only and are absent here. A shape binds nothing: a
    /// `Quantified` inside it reads the enclosing [`Self::Scheme`]'s group.
    ExpressionShape {
        /// The call shape: fixed keywords interleaved with the argument positions' declared types.
        elements: Elements<'run, H>,
        /// Each slot's priority class, in slot order, dense from 0 — the ranking a dispatch admits
        /// and ranks the slots by, class by class. Written order reads `0..n`, and the digest
        /// feeds nothing for it.
        classes: Classes<'run>,
        ret: H,
    },
    /// A **scheme** — a quantified callable's type, and the one binder: `group` binds the free
    /// `Quantified` positions of `body`, a [`Self::KFunction`] or a [`Self::ExpressionShape`]. The
    /// body's node says which. The group is non-empty: the two scheme doors answer the plain body
    /// for an empty one. The digest feeds the group's arity and bounds and the body's digest, never
    /// the names, so alpha-variants intern once and `group` holds whichever spelling was interned
    /// first. Build through
    /// [`TypeChain::function_scheme`](super::chain::TypeChain::function_scheme) or
    /// [`TypeChain::shape_scheme`](super::chain::TypeChain::shape_scheme).
    Scheme {
        group: &'run Group<'run>,
        body: H,
    },
    /// A **rigid variable bound by the enclosing [`Self::Scheme`]**: the `index`-th member of its
    /// group, bounded by `bound`. Positional rather than named, so two schemes alpha-equivalent
    /// under a renaming of their parameters intern to one node.
    Quantified {
        index: usize,
        bound: H::Bound,
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
        lower: H::Bound,
        bound: H::Bound,
    },
    /// Untagged structural disjunction — the type `:(A | B)`. Members are canonical:
    /// deduplicated, no nested `Union`, no member below the rest, always two or more, in the order
    /// first written. Identity is order-blind. Build through
    /// [`TypeChain::union_of`](super::chain::TypeChain::union_of).
    Union {
        members: Run<'run, H>,
    },
    /// Application of a higher-kinded type constructor to argument types. `arguments` maps each
    /// of the constructor's parameter names to the elaborated argument type; the digest feeds
    /// them name-sorted, so the same name-to-type map is the same application however written.
    ConstructorApply {
        constructor: H,
        arguments: Record<'run, H>,
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
    /// [`TypeChain::signature_apply`](super::chain::TypeChain::signature_apply), which
    /// answers `signature` itself for no pins.
    SignatureApply {
        signature: H::Bound,
        pins: Record<'run, H>,
    },
    /// A **set of applications**, two or more, none lying above another, sorted by handle so the
    /// set's identity is order-blind: what the meet of two signature types is. Each member is a
    /// [`Self::Signature`] or a [`Self::SignatureApply`], never the empty signature and never a
    /// meet. Build through
    /// [`TypeChain::signature_meet`](super::chain::TypeChain::signature_meet).
    SignatureMeet {
        members: Run<'run, H>,
    },
    /// Confined carrier for a synthesized FN `ret` slot whose source return is deferred. Holds
    /// only the hashable surface shadow, and admits nothing on its own.
    DeferredReturn(DeferredReturnSurface<'run>),
    /// A relative sibling reference inside a pre-seal recursive-group window: the sibling's bare
    /// index, meaningful only against the ambient window. Ordinary interned content, and the
    /// order relates it as an atom with the same profile as the member it will become — which is
    /// what lets a relative schema's unions canonicalize before their members exist. A sealed
    /// [`Component`] holds its schemas in this relative form only while it is tied; an absolute
    /// schema never holds one.
    Sibling(usize),
    /// One sealed member of a recursive group. Identity is its strongly-connected component's
    /// digest plus its index in that component's canonical order — the numeric order of the
    /// members' name symbols — so two independently built components with the same content intern
    /// to the same nodes.
    ///
    /// `name`, `kind` and the schema are excluded from the digest because the component's digest
    /// was computed over them — a handle determines them. The schema is read absolute through
    /// [`TypeChain::member_schema`](super::chain::TypeChain::member_schema).
    SetMember {
        component: &'run Component<'run>,
        index: usize,
        name: TypeSymbol,
        kind: KKind,
    },
}

impl<'run, H: Naming<'run>> TypeNode<'run, H> {
    /// The quantifier group a scheme binds, or `None` for every other node.
    pub fn group(&self) -> Option<&'run Group<'run>> {
        match self {
            TypeNode::Scheme { group, .. } => Some(group),
            // No wildcard: a new binder variant must say so here, or every walk reads its own
            // variables as free.
            TypeNode::Number
            | TypeNode::Str
            | TypeNode::Bool
            | TypeNode::Null
            | TypeNode::Identifier
            | TypeNode::Symbol
            | TypeNode::TypeNameToken
            | TypeNode::Expression
            | TypeNode::SigiledTypeExpr
            | TypeNode::RecordType
            | TypeNode::Literal
            | TypeNode::Block
            | TypeNode::Declaration
            | TypeNode::Binder
            | TypeNode::Name
            | TypeNode::Keyword
            | TypeNode::Any
            | TypeNode::AnyValue
            | TypeNode::AnyCode
            | TypeNode::Never
            | TypeNode::OfKind(_)
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
            | TypeNode::Sibling(_) => None,
        }
    }

    /// Whether this node binds a quantifier group: a scheme, and nothing else. Every walk asks this
    /// before stepping into a child, so a `Quantified` under it reads against the right group.
    pub fn binds_quantifiers(&self) -> bool {
        self.group().is_some()
    }

    /// A rigid variable's bound — a [`Self::Quantified`]'s, a [`Self::Lexical`]'s or a
    /// [`Self::Parameter`]'s — or `None` for any other node.
    pub fn rigid_bound(&self) -> Option<H::Bound> {
        Variable::of(self).map(Variable::bound)
    }
}

impl<'run, H: TypeHandle> TypeNode<'run, H> {
    /// A rigid variable's lower end: a [`Self::Lexical`]'s own, `Never` for the other two, `None`
    /// for any other node.
    pub fn rigid_lower(&self) -> Option<KType> {
        Variable::of(self).map(Variable::lower)
    }

    /// A rigid variable's two ends as an interval, or `None` for any other node.
    pub fn rigid_interval(&self) -> Option<Interval<KType>> {
        Variable::of(self).map(Variable::interval)
    }
}

impl<'run> TypeNode<'run, Linked<'run>> {
    /// [`rigid_lower`](TypeNode::rigid_lower) by reference: `Never` is the chain's own node.
    pub(super) fn rigid_lower_in(
        &self,
        types: &TypeChain<'run, impl Habitat<'run>>,
    ) -> Option<Linked<'run>> {
        Variable::of(self).map(|variable| variable.lower_in(types))
    }

    /// [`rigid_interval`](TypeNode::rigid_interval) by reference.
    pub(super) fn rigid_interval_in(
        &self,
        types: &TypeChain<'run, impl Habitat<'run>>,
    ) -> Option<Interval<Linked<'run>>> {
        Variable::of(self).map(|variable| variable.interval_in(types))
    }
}

/// A free variable a read through intervals meets, as its node spells it, its ends read as `B`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variable<B = KType> {
    /// A scheme's own variable, read where no scheme captures it.
    Quantified { index: usize, bound: B },
    /// A lexical variable at `level` along the chain of bodies that declares it.
    Lexical {
        level: usize,
        name: TypeSymbol,
        lower: B,
        bound: B,
    },
    /// A signature's head parameter.
    Parameter { name: TypeSymbol, bound: B },
}

impl<B: Copy> Variable<B> {
    /// The variable `node` is, or `None` for any other node.
    pub(super) fn of<'run, H: Naming<'run, Bound = B>>(node: &TypeNode<'run, H>) -> Option<Self> {
        Some(match *node {
            TypeNode::Quantified { index, bound } => Variable::Quantified { index, bound },
            TypeNode::Lexical {
                level,
                name,
                lower,
                bound,
            } => Variable::Lexical {
                level,
                name,
                lower,
                bound,
            },
            TypeNode::Parameter { name, bound } => Variable::Parameter { name, bound },
            _ => return None,
        })
    }

    /// The variable's bound.
    pub fn bound(self) -> B {
        match self {
            Variable::Quantified { bound, .. }
            | Variable::Lexical { bound, .. }
            | Variable::Parameter { bound, .. } => bound,
        }
    }
}

impl Variable {
    /// The variable's lower end: a lexical variable's own, `Never` for the other two.
    pub fn lower(self) -> KType {
        match self {
            Variable::Lexical { lower, .. } => lower,
            Variable::Quantified { .. } | Variable::Parameter { .. } => KType::NEVER,
        }
    }

    /// The variable's two ends.
    pub fn interval(self) -> Interval<KType> {
        Interval {
            lower: self.lower(),
            upper: self.bound(),
        }
    }
}

impl<'run> Variable<Linked<'run>> {
    /// The variable with its ends read as handles.
    pub(super) fn typed(self) -> Variable {
        match self {
            Variable::Quantified { index, bound } => Variable::Quantified {
                index,
                bound: KType::read(bound),
            },
            Variable::Lexical {
                level,
                name,
                lower,
                bound,
            } => Variable::Lexical {
                level,
                name,
                lower: KType::read(lower),
                bound: KType::read(bound),
            },
            Variable::Parameter { name, bound } => Variable::Parameter {
                name,
                bound: KType::read(bound),
            },
        }
    }

    /// [`lower`](Variable::lower) by reference: `Never` is the chain's own node.
    pub(super) fn lower_in(self, types: &TypeChain<'run, impl Habitat<'run>>) -> Linked<'run> {
        match self {
            Variable::Lexical { lower, .. } => lower,
            Variable::Quantified { .. } | Variable::Parameter { .. } => types.never(),
        }
    }

    /// [`interval`](Variable::interval) by reference.
    pub(super) fn interval_in(
        self,
        types: &TypeChain<'run, impl Habitat<'run>>,
    ) -> Interval<Linked<'run>> {
        Interval {
            lower: self.lower_in(types),
            upper: self.bound(),
        }
    }
}

/// A sealed member's schema, over absolute member handles: every sibling reference inside it is
/// the sibling's own handle, which is what makes a group's composition edges cyclic. The pre-seal
/// window carries the relative twin of this shape.
#[derive(Clone, Copy)]
pub enum NodeSchema<'run> {
    /// Fresh nominal over a transparent representation.
    NewType(KType),
    /// Higher-kinded constructor: the representation a construction through the family wraps,
    /// written over `Quantified { index, bound: Any }` for `param_names[index]`, or `None` for a
    /// family that constructs nothing; plus its parameter names, Type-class symbols interned at
    /// the declaration, stored symbol-sorted because a constructor's identity is their set.
    TypeConstructor {
        representation: Option<Parametric>,
        param_names: &'run [TypeSymbol],
    },
}
