//! [`StoredNode`] — one interned type as the registry's table stores it: its digest beside its
//! [`Content`]. Each node is laid alone in the region, and the table holds a reference to it.
//!
//! The table stores a node apart from the [`TypeNode`] a reader sees, so storage can pack what a
//! view spells out. Each variant whose probe answers its children decide carries a [`Flags`] byte:
//! the four answers, folded at intern so each probe is one read of the node. Every other
//! variant's answers are a constant of its kind, which [`Content::flags`] answers.
//!
//! A few fields a hot path never reads sit behind one `&'run` reference, laid in the region on an
//! intern miss: a carrier's key and name, a lexical variable's name, a signature's schema, a
//! deferred return's surface, and a sealed member's [`Component`]. A lexical level and a member's
//! index are stored narrowed to `u32`. A shape stores each slot's priority class in that
//! slot's element, where the view reads its ranking. A reader that needs only inline fields, such
//! as [`kind_of`](super::handle::KType::kind_of), matches the stored content and skips the hop.
//!
//! Each child, and each concrete field — a bound, a lower end, a code kind, a carrier's met bound,
//! an application's signature — is a [`Linked`] reference to its node, so a walk steps from node to
//! node without the table. A group's bounds and a schema's members stay handles, which koan reads.
//!
//! Two conversions connect the two shapes, each one exhaustive match: [`Content::lay`] lays a
//! view over references as stored content on an intern miss, and [`StoredNode::view_as`] reads a
//! node back as a view, each child read as the reader names it — by its handle outside the lattice,
//! by reference inside ([`StoredNode::view`]). Every reader outside the lattice goes through
//! [`TypeRegistry::node`](super::registry::TypeRegistry::node) and never sees this layout.
//!
//! See [README.md](README.md) § The node vocabulary.

use std::fmt;
use std::hash::{Hash, Hasher};

use crate::bump::BumpAllocator;
use crate::symbols::{BinderSymbol, TypeSymbol};

use super::component::Component;
use super::digest::TypeDigest;
use super::handle::{DeclaredType, Handle, Naming, Parametric, wrap};
use super::kind::KKind;
use super::node::{ContentKey, Group, TypeNode};
use super::record::Record;
use super::run::{Elements, Run};
use super::schema::SigSchema;
use super::shape::DeferredReturnSurface;

/// The four probe answers, folded from a node's children when it is interned: a free
/// `Quantified`, any rigid variable, anything parametric, an opaque carrier.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Flags(u8);

impl Flags {
    pub(super) const NONE: Self = Flags(0);
    /// A `Quantified` position is reachable without crossing a scheme.
    pub(super) const QUANTIFIED: Self = Flags(1);
    /// A rigid variable — `Quantified`, `Lexical` or `Parameter` — is reachable.
    pub(super) const RIGID: Self = Flags(2);
    /// A free `Quantified`, a `Lexical`, a head `Parameter` or a scheme is reachable outside
    /// sealed content: the type is parametric rather than concrete.
    pub(super) const PARAMETRIC: Self = Flags(4);
    /// An opaque carrier is reachable outside sealed content.
    pub(super) const CARRIER: Self = Flags(8);

    /// These flags less `other`'s.
    pub(super) fn without(self, other: Self) -> Self {
        Flags(self.0 & !other.0)
    }

    /// Whether every flag of `other` is set here.
    pub(super) fn holds(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Flags(self.0 | other.0)
    }
}

/// A stored node's content: [`TypeNode`]'s variants in its order, each child and each concrete
/// field a reference to its node, and a [`Flags`] byte in each variant whose children decide its
/// probe answers.
#[derive(Clone, Copy)]
pub(super) enum Content<'run> {
    Number,
    Str,
    Bool,
    Null,
    Identifier,
    Symbol,
    TypeNameToken,
    Expression,
    SigiledTypeExpr,
    RecordType,
    Literal,
    Block,
    Declaration,
    Binder,
    Name,
    Keyword,
    Any,
    AnyValue,
    AnyCode,
    CodeNeeding {
        kind: Linked<'run>,
        names: &'run [BinderSymbol],
    },
    Never,
    OfKind(KKind),
    Parameter {
        name: TypeSymbol,
        bound: Linked<'run>,
    },
    Carrier {
        /// The carrier's key and name, laid together.
        key: &'run (ContentKey, TypeSymbol),
        met: Linked<'run>,
    },
    List {
        flags: Flags,
        element: Linked<'run>,
    },
    Dict {
        flags: Flags,
        key: Linked<'run>,
        value: Linked<'run>,
    },
    Record {
        flags: Flags,
        fields: Record<'run, Linked<'run>>,
    },
    KFunction {
        flags: Flags,
        params: Record<'run, Linked<'run>>,
        ret: Linked<'run>,
    },
    ExpressionShape {
        flags: Flags,
        elements: Elements<'run, Linked<'run>>,
        ret: Linked<'run>,
    },
    Scheme {
        flags: Flags,
        group: &'run Group<'run>,
        body: Linked<'run>,
    },
    Quantified {
        index: usize,
        bound: Linked<'run>,
    },
    Lexical {
        level: u32,
        name: &'run TypeSymbol,
        lower: Linked<'run>,
        bound: Linked<'run>,
    },
    Union {
        flags: Flags,
        members: Run<'run, Linked<'run>>,
    },
    ConstructorApply {
        flags: Flags,
        constructor: Linked<'run>,
        arguments: Record<'run, Linked<'run>>,
    },
    Signature {
        schema: &'run SigSchema<'run>,
        schema_digest: TypeDigest,
    },
    SignatureApply {
        flags: Flags,
        signature: Linked<'run>,
        pins: Record<'run, Linked<'run>>,
    },
    SignatureMeet {
        flags: Flags,
        members: Run<'run, Linked<'run>>,
    },
    DeferredReturn(&'run DeferredReturnSurface<'run>),
    Sibling(usize),
    SetMember {
        kind: KKind,
        index: u32,
        component: &'run Component<'run>,
        name: TypeSymbol,
    },
}

/// A node named by reference: what the table holds, what a node names its children by, and what
/// every walk and relation inside the lattice reads.
pub type Linked<'run> = &'run StoredNode<'run>;

/// One interned node as the table stores it: its digest, then its content. Laid alone in the
/// registry's region on an intern miss, so a reference to it lives as long as the region.
/// Nominally `pub` so the sealed reading trait can name it; `stored` is private, so nothing
/// outside the lattice can.
///
/// Two nodes compare, order and hash by digest. A reference is an identity only within one table,
/// and a digest is one everywhere.
pub struct StoredNode<'run> {
    digest: TypeDigest,
    content: Content<'run>,
}

// A node fills 48 B — its digest and 32 B of content — and the region releases it without running
// anything.
const _: () = assert!(std::mem::size_of::<Content<'static>>() == 32);
const _: () = assert!(std::mem::size_of::<StoredNode<'static>>() == 48);
const _: () = assert!(std::mem::align_of::<StoredNode<'static>>() == 16);
const _: () = assert!(!std::mem::needs_drop::<StoredNode<'static>>());

impl<'run> StoredNode<'run> {
    /// `content` under `digest`, ready to lay in the region.
    pub(super) fn new(digest: TypeDigest, content: Content<'run>) -> Self {
        StoredNode { digest, content }
    }

    /// The digest the node is interned under.
    pub(super) fn digest(&self) -> TypeDigest {
        self.digest
    }

    /// The node's handle: its digest, as a handle.
    pub(super) fn handle(&self) -> Handle {
        Handle::from_digest(self.digest)
    }

    /// The node's content, by value.
    pub(super) fn content(&self) -> Content<'run> {
        self.content
    }

    /// The node's probe answers.
    pub(super) fn flags(&self) -> Flags {
        self.content.flags()
    }

    /// This node as the lattice reads it: every child and every concrete field by reference.
    pub(super) fn view(&self) -> TypeNode<'run, Linked<'run>> {
        self.view_as()
    }

    /// This node with its children read as `C` and its concrete fields as `C::Bound` — **the view
    /// table**, one exhaustive match, so a new variant is a compile error here. The caller answers
    /// for `C`'s promise: the registry reads a node as the handle it was named by, and a scheme's
    /// positions as [`Parametric`](super::handle::Parametric).
    pub(super) fn view_as<C: Naming<'run>>(&self) -> TypeNode<'run, C> {
        match self.content {
            Content::Number => TypeNode::Number,
            Content::Str => TypeNode::Str,
            Content::Bool => TypeNode::Bool,
            Content::Null => TypeNode::Null,
            Content::Identifier => TypeNode::Identifier,
            Content::Symbol => TypeNode::Symbol,
            Content::TypeNameToken => TypeNode::TypeNameToken,
            Content::Expression => TypeNode::Expression,
            Content::SigiledTypeExpr => TypeNode::SigiledTypeExpr,
            Content::RecordType => TypeNode::RecordType,
            Content::Literal => TypeNode::Literal,
            Content::Block => TypeNode::Block,
            Content::Declaration => TypeNode::Declaration,
            Content::Binder => TypeNode::Binder,
            Content::Name => TypeNode::Name,
            Content::Keyword => TypeNode::Keyword,
            Content::Any => TypeNode::Any,
            Content::AnyValue => TypeNode::AnyValue,
            Content::AnyCode => TypeNode::AnyCode,
            Content::CodeNeeding { kind, names } => TypeNode::CodeNeeding {
                kind: C::read_bound(kind),
                names,
            },
            Content::Never => TypeNode::Never,
            Content::OfKind(kind) => TypeNode::OfKind(kind),
            Content::Parameter { name, bound } => TypeNode::Parameter {
                name,
                bound: C::read_bound(bound),
            },
            Content::Carrier { key, met } => TypeNode::Carrier {
                name: key.1,
                key: key.0,
                met: C::read_bound(met),
            },
            Content::List { element, .. } => TypeNode::List {
                element: C::read(element),
            },
            Content::Dict { key, value, .. } => TypeNode::Dict {
                key: C::read(key),
                value: C::read(value),
            },
            Content::Record { fields, .. } => TypeNode::Record {
                fields: Record::over(fields.raw()),
            },
            Content::KFunction { params, ret, .. } => TypeNode::KFunction {
                params: Record::over(params.raw()),
                ret: C::read(ret),
            },
            Content::ExpressionShape { elements, ret, .. } => TypeNode::ExpressionShape {
                elements: Elements::over(elements.raw()),
                classes: elements.classes(),
                ret: C::read(ret),
            },
            Content::Scheme { group, body, .. } => TypeNode::Scheme {
                group,
                body: C::read(body),
            },
            Content::Quantified { index, bound } => TypeNode::Quantified {
                index,
                bound: C::read_bound(bound),
            },
            Content::Lexical {
                level,
                name,
                lower,
                bound,
            } => TypeNode::Lexical {
                level: level as usize,
                name: *name,
                lower: C::read_bound(lower),
                bound: C::read_bound(bound),
            },
            Content::Union { members, .. } => TypeNode::Union {
                members: Run::over(members.raw()),
            },
            Content::ConstructorApply {
                constructor,
                arguments,
                ..
            } => TypeNode::ConstructorApply {
                constructor: C::read(constructor),
                arguments: Record::over(arguments.raw()),
            },
            Content::Signature {
                schema,
                schema_digest,
            } => TypeNode::Signature {
                schema: *schema,
                schema_digest,
            },
            Content::SignatureApply {
                signature, pins, ..
            } => TypeNode::SignatureApply {
                signature: C::read_bound(signature),
                pins: Record::over(pins.raw()),
            },
            Content::SignatureMeet { members, .. } => TypeNode::SignatureMeet {
                members: Run::over(members.raw()),
            },
            Content::DeferredReturn(surface) => TypeNode::DeferredReturn(*surface),
            Content::Sibling(index) => TypeNode::Sibling(index),
            Content::SetMember {
                kind,
                index,
                component,
                name,
            } => TypeNode::SetMember {
                component,
                index: index as usize,
                name,
                kind,
            },
        }
    }

    /// The node's own quantifier group and the type under it: a scheme's group and body, or the
    /// empty group over the node itself. How the lattice's relations read a callable that may be a
    /// scheme.
    pub(super) fn split(&'run self) -> (Group<'run>, Linked<'run>) {
        match self.content {
            Content::Scheme { group, body, .. } => (*group, body),
            _ => (Group::EMPTY, self),
        }
    }

    /// What the node is declared as: a [`Scheme`](super::handle::Scheme) where it is a scheme
    /// node, and a [`Parametric`] otherwise.
    pub(super) fn declared(&self) -> DeclaredType<Parametric> {
        if matches!(self.content, Content::Scheme { .. }) {
            DeclaredType::Scheme(wrap(self.handle()))
        } else {
            DeclaredType::Type(wrap(self.handle()))
        }
    }

    /// Whether any `Quantified` position is reachable from the node without crossing a scheme —
    /// the probe that lets a slot type with nothing to solve answer the relations in one step
    /// instead of walking under a unifier. Read off the flags interning stored in the node.
    pub(super) fn contains_quantified(&self) -> bool {
        self.flags().holds(Flags::QUANTIFIED)
    }

    /// Whether any rigid variable — `Quantified`, `Lexical` or `Parameter` — is reachable from the
    /// node. Read off the flags interning stored in the node.
    ///
    /// The invariant a **bound** carries: a bound is a variable-free type. That is what keeps the
    /// order's two rigid clauses consistent, since below a rigid variable are only itself and
    /// `Never` while above it is everything above its bound — and a rigid bound would put a
    /// variable in both sets at once. A bound is a [`KType`](super::handle::KType), so it holds no variable; the doors
    /// that take a bound assert it holds no opaque carrier either.
    pub(super) fn contains_rigid(&self) -> bool {
        self.flags().holds(Flags::RIGID)
    }

    /// Whether the node is concrete: no free `Quantified`, `Lexical`, head `Parameter` or scheme is
    /// reachable outside sealed content. Read off the flags interning stored in the node.
    pub(super) fn is_concrete(&self) -> bool {
        !self.flags().holds(Flags::PARAMETRIC)
    }

    /// Whether an opaque carrier is reachable from the node outside sealed content. Read off the
    /// flag interning stored in the node.
    pub(super) fn contains_carrier(&self) -> bool {
        self.flags().holds(Flags::CARRIER)
    }
}

impl PartialEq for StoredNode<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}

impl Eq for StoredNode<'_> {}

impl PartialOrd for StoredNode<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StoredNode<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.digest.cmp(&other.digest)
    }
}

impl Hash for StoredNode<'_> {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.digest.hash(state);
    }
}

/// A node compares against a handle by digest, so a reference reads against a constant.
impl PartialEq<Handle> for &StoredNode<'_> {
    fn eq(&self, other: &Handle) -> bool {
        self.digest == other.digest()
    }
}

impl fmt::Debug for StoredNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Linked(0x{:032x})", self.digest.0)
    }
}

impl<'run> Content<'run> {
    /// `node` as the table stores it, carrying `flags` where its variant does. Its slices already
    /// live in the region; a carrier's key and name and a deferred return's surface are laid out of
    /// line here.
    pub(super) fn lay(
        node: TypeNode<'run, Linked<'run>>,
        flags: Flags,
        bump: BumpAllocator<'run>,
    ) -> Self {
        match node {
            TypeNode::Number => Content::Number,
            TypeNode::Str => Content::Str,
            TypeNode::Bool => Content::Bool,
            TypeNode::Null => Content::Null,
            TypeNode::Identifier => Content::Identifier,
            TypeNode::Symbol => Content::Symbol,
            TypeNode::TypeNameToken => Content::TypeNameToken,
            TypeNode::Expression => Content::Expression,
            TypeNode::SigiledTypeExpr => Content::SigiledTypeExpr,
            TypeNode::RecordType => Content::RecordType,
            TypeNode::Literal => Content::Literal,
            TypeNode::Block => Content::Block,
            TypeNode::Declaration => Content::Declaration,
            TypeNode::Binder => Content::Binder,
            TypeNode::Name => Content::Name,
            TypeNode::Keyword => Content::Keyword,
            TypeNode::Any => Content::Any,
            TypeNode::AnyValue => Content::AnyValue,
            TypeNode::AnyCode => Content::AnyCode,
            TypeNode::CodeNeeding { kind, names } => Content::CodeNeeding { kind, names },
            TypeNode::Never => Content::Never,
            TypeNode::OfKind(kind) => Content::OfKind(kind),
            TypeNode::Parameter { name, bound } => Content::Parameter { name, bound },
            TypeNode::Carrier { name, key, met } => Content::Carrier {
                key: bump.alloc((key, name)),
                met,
            },
            TypeNode::List { element } => Content::List { flags, element },
            TypeNode::Dict { key, value } => Content::Dict { flags, key, value },
            TypeNode::Record { fields } => Content::Record { flags, fields },
            TypeNode::KFunction { params, ret } => Content::KFunction { flags, params, ret },
            TypeNode::ExpressionShape {
                elements,
                classes,
                ret,
            } => {
                debug_assert!(
                    classes == elements.classes(),
                    "a shape's classes ride its elements"
                );
                Content::ExpressionShape {
                    flags,
                    elements,
                    ret,
                }
            }
            TypeNode::Scheme { group, body } => Content::Scheme { flags, group, body },
            TypeNode::Quantified { index, bound } => Content::Quantified { index, bound },
            TypeNode::Lexical {
                level,
                name,
                lower,
                bound,
            } => Content::Lexical {
                level: u32::try_from(level).expect("a lexical level fits u32"),
                name: bump.alloc(name),
                lower,
                bound,
            },
            TypeNode::Union { members } => Content::Union { flags, members },
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => Content::ConstructorApply {
                flags,
                constructor,
                arguments,
            },
            TypeNode::Signature {
                schema,
                schema_digest,
            } => Content::Signature {
                schema: bump.alloc(schema),
                schema_digest,
            },
            TypeNode::SignatureApply { signature, pins } => Content::SignatureApply {
                flags,
                signature,
                pins,
            },
            TypeNode::SignatureMeet { members } => Content::SignatureMeet { flags, members },
            TypeNode::DeferredReturn(surface) => Content::DeferredReturn(bump.alloc(surface)),
            TypeNode::Sibling(index) => Content::Sibling(index),
            TypeNode::SetMember {
                component,
                index,
                name,
                kind,
            } => Content::SetMember {
                kind,
                index: u32::try_from(index).expect("a component has fewer than 2^32 members"),
                component,
                name,
            },
        }
    }

    /// The node's probe answers — the one door every probe reads. A variant with a [`Flags`] field
    /// answers it; every other variant answers a constant of its kind. A variable's are constant
    /// because its bound and lower end are concrete and carrier-free.
    pub(super) fn flags(&self) -> Flags {
        match *self {
            Content::List { flags, .. }
            | Content::Dict { flags, .. }
            | Content::Record { flags, .. }
            | Content::KFunction { flags, .. }
            | Content::ExpressionShape { flags, .. }
            | Content::Scheme { flags, .. }
            | Content::Union { flags, .. }
            | Content::ConstructorApply { flags, .. }
            | Content::SignatureApply { flags, .. }
            | Content::SignatureMeet { flags, .. } => flags,
            Content::Quantified { .. } => Flags::QUANTIFIED | Flags::RIGID | Flags::PARAMETRIC,
            Content::Lexical { .. } | Content::Parameter { .. } => Flags::RIGID | Flags::PARAMETRIC,
            // An opaque carrier: a value carries it and dispatches on it, so it is concrete.
            Content::Carrier { .. } => Flags::CARRIER,
            // Sealed content and leaves: every walk treats them as leaves.
            Content::Number
            | Content::Str
            | Content::Bool
            | Content::Null
            | Content::Identifier
            | Content::Symbol
            | Content::TypeNameToken
            | Content::Expression
            | Content::SigiledTypeExpr
            | Content::RecordType
            | Content::Literal
            | Content::Block
            | Content::Declaration
            | Content::Binder
            | Content::Name
            | Content::Keyword
            | Content::Any
            | Content::AnyValue
            | Content::AnyCode
            | Content::CodeNeeding { .. }
            | Content::Never
            | Content::OfKind(_)
            | Content::Signature { .. }
            | Content::DeferredReturn(_)
            | Content::Sibling(_)
            | Content::SetMember { .. } => Flags::NONE,
        }
    }
}
