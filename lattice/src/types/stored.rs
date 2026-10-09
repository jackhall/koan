//! [`StoredNode`] — one interned type as the registry's table stores it: its digest beside its
//! [`Content`]. Each node is laid alone in the region, and the table holds a reference to it.
//!
//! The table stores a node apart from the [`TypeNode`] a reader sees, so storage can pack what a
//! view spells out. Each variant whose probe answers its children decide carries a [`Flags`] byte:
//! the four answers, folded at intern so each probe is one read of the node. Every other
//! variant's answers are a constant of its kind, which [`Content::flags`] answers.
//!
//! A few fields a hot path never reads sit behind one `&'run` reference, laid in the region on an
//! intern miss: a carrier's key, a lexical variable's name, a signature's schema, and a sealed
//! member's component digest and schema, the schema in a slot its seal sets. A lexical level and a
//! member's index are stored narrowed to `u32`. A shape stores each slot's priority class in that
//! slot's element, where the view reads its ranking. A reader that needs only inline fields, such as [`kind_of`](super::handle::KType::kind_of),
//! matches the stored content and skips the hop.
//!
//! Two conversions connect the two shapes, each one exhaustive match: [`Content::lay`] lays a
//! view as stored content on an intern miss, and [`Content::view`] reads stored content back as a
//! view, its children rewrapped as the handle the reader named it by. Every other reader goes
//! through [`TypeRegistry::node`](super::registry::TypeRegistry::node) and never sees this layout.
//!
//! See [README.md](README.md) § The node vocabulary.

use std::cell::Cell;

use crate::bump::BumpAllocator;
use crate::symbols::{BinderSymbol, TypeSymbol};

use super::digest::TypeDigest;
use super::handle::{Handle, KType, TypeHandle, wrap};
use super::kind::KKind;
use super::node::{ContentKey, Group, NodeSchema, TypeNode};
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

/// A stored node's content: [`TypeNode`]'s variants in its order, children raw, and a [`Flags`]
/// byte in each variant whose children decide its probe answers.
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
        kind: KType,
        names: &'run [BinderSymbol],
    },
    Never,
    OfKind(KKind),
    Parameter {
        name: TypeSymbol,
        bound: KType,
    },
    Carrier {
        key: &'run ContentKey,
        name: TypeSymbol,
        met: KType,
    },
    List {
        flags: Flags,
        element: Handle,
    },
    Dict {
        flags: Flags,
        key: Handle,
        value: Handle,
    },
    Record {
        flags: Flags,
        fields: Record<'run, Handle>,
    },
    KFunction {
        flags: Flags,
        params: Record<'run, Handle>,
        ret: Handle,
    },
    ExpressionShape {
        flags: Flags,
        elements: Elements<'run, Handle>,
        ret: Handle,
    },
    Scheme {
        flags: Flags,
        group: &'run Group<'run>,
        body: Handle,
    },
    Quantified {
        index: usize,
        bound: KType,
    },
    Lexical {
        level: u32,
        name: &'run TypeSymbol,
        lower: KType,
        bound: KType,
    },
    Union {
        flags: Flags,
        members: Run<'run, Handle>,
    },
    ConstructorApply {
        flags: Flags,
        constructor: Handle,
        arguments: Record<'run, Handle>,
    },
    Signature {
        schema: &'run SigSchema<'run>,
        schema_digest: TypeDigest,
    },
    SignatureApply {
        flags: Flags,
        signature: KType,
        pins: Record<'run, Handle>,
    },
    SignatureMeet {
        flags: Flags,
        members: Run<'run, Handle>,
    },
    DeferredReturn(DeferredReturnSurface<'run>),
    Sibling(usize),
    SetMember {
        kind: KKind,
        index: u32,
        body: &'run MemberBody<'run>,
        name: TypeSymbol,
    },
}

/// What a sealed member carries out of line: the digest of its component, which only a re-digest
/// of the node reads, and its schema's slot.
pub(super) struct MemberBody<'run> {
    pub(super) scc_digest: TypeDigest,
    pub(super) schema: SchemaSlot<'run>,
}

const _: () = assert!(!std::mem::needs_drop::<MemberBody<'static>>());

/// A sealed member's schema. The seal lays the slot empty when it interns the member and sets it
/// once, after it has rewritten the schema absolute: the schema names types over the member, which
/// can only be interned once the member is. Nothing sets one twice.
pub(super) struct SchemaSlot<'run>(Cell<Option<NodeSchema<'run>>>);

impl<'run> SchemaSlot<'run> {
    /// A slot holding `schema` from the start.
    pub(super) fn filled(schema: NodeSchema<'run>) -> Self {
        SchemaSlot(Cell::new(Some(schema)))
    }

    /// The schema the seal set.
    pub(super) fn get(&self) -> NodeSchema<'run> {
        self.0
            .get()
            .expect("a sealed member's schema is set by its seal before anything reads it")
    }
}

/// A node named by reference: what the table holds, and what every walk and relation inside the
/// lattice reads.
pub(super) type Linked<'run> = &'run StoredNode<'run>;

/// One interned node as the table stores it: its digest, then its content. Laid alone in the
/// registry's region on an intern miss, so a reference to it lives as long as the region.
pub(super) struct StoredNode<'run> {
    digest: TypeDigest,
    content: Content<'run>,
}

// A node fills 64 B — its digest and 48 B of content — and the region releases it without running
// anything.
const _: () = assert!(std::mem::size_of::<Content<'static>>() == 48);
const _: () = assert!(std::mem::size_of::<StoredNode<'static>>() == 64);
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

    /// The node's content, by value.
    pub(super) fn content(&self) -> Content<'run> {
        self.content
    }

    /// The node's probe answers.
    pub(super) fn flags(&self) -> Flags {
        self.content.flags()
    }

    /// The node read back as a view, its children read as `C`.
    pub(super) fn view<C: TypeHandle>(&self) -> TypeNode<'run, C> {
        self.content.view()
    }
}

impl<'run> Content<'run> {
    /// `node` as the table stores it, carrying `flags` where its variant does. Its slices already
    /// live in the region.
    pub(super) fn lay(node: TypeNode<'run>, flags: Flags, bump: BumpAllocator<'run>) -> Self {
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
                key: bump.alloc(key),
                name,
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
            TypeNode::DeferredReturn(surface) => Content::DeferredReturn(surface),
            TypeNode::Sibling(index) => Content::Sibling(index),
            TypeNode::SetMember {
                scc_digest,
                index,
                name,
                kind,
                schema,
            } => Content::SetMember {
                kind,
                index: u32::try_from(index).expect("a component has fewer than 2^32 members"),
                body: bump.alloc(MemberBody {
                    scc_digest,
                    schema: SchemaSlot::filled(schema),
                }),
                name,
            },
        }
    }

    /// This node as a view with its children read as `C` — **the view table**, one exhaustive
    /// match, so a new variant is a compile error here. The caller answers for `C`'s promise: the
    /// registry reads a node as the handle it was named by, and a scheme's positions as
    /// [`Parametric`](super::handle::Parametric).
    pub(super) fn view<C: TypeHandle>(self) -> TypeNode<'run, C> {
        let child = |h: Handle| wrap::<C>(h);
        match self {
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
            Content::CodeNeeding { kind, names } => TypeNode::CodeNeeding { kind, names },
            Content::Never => TypeNode::Never,
            Content::OfKind(kind) => TypeNode::OfKind(kind),
            Content::Parameter { name, bound } => TypeNode::Parameter { name, bound },
            Content::Carrier { key, name, met } => TypeNode::Carrier {
                name,
                key: *key,
                met,
            },
            Content::List { element, .. } => TypeNode::List {
                element: child(element),
            },
            Content::Dict { key, value, .. } => TypeNode::Dict {
                key: child(key),
                value: child(value),
            },
            Content::Record { fields, .. } => TypeNode::Record {
                fields: Record::over(fields.raw()),
            },
            Content::KFunction { params, ret, .. } => TypeNode::KFunction {
                params: Record::over(params.raw()),
                ret: child(ret),
            },
            Content::ExpressionShape { elements, ret, .. } => TypeNode::ExpressionShape {
                elements: Elements::over(elements.raw()),
                classes: elements.classes(),
                ret: child(ret),
            },
            Content::Scheme { group, body, .. } => TypeNode::Scheme {
                group,
                body: child(body),
            },
            Content::Quantified { index, bound } => TypeNode::Quantified { index, bound },
            Content::Lexical {
                level,
                name,
                lower,
                bound,
            } => TypeNode::Lexical {
                level: level as usize,
                name: *name,
                lower,
                bound,
            },
            Content::Union { members, .. } => TypeNode::Union {
                members: Run::over(members.raw()),
            },
            Content::ConstructorApply {
                constructor,
                arguments,
                ..
            } => TypeNode::ConstructorApply {
                constructor: child(constructor),
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
                signature,
                pins: Record::over(pins.raw()),
            },
            Content::SignatureMeet { members, .. } => TypeNode::SignatureMeet {
                members: Run::over(members.raw()),
            },
            Content::DeferredReturn(surface) => TypeNode::DeferredReturn(surface),
            Content::Sibling(index) => TypeNode::Sibling(index),
            Content::SetMember {
                kind,
                index,
                body,
                name,
            } => TypeNode::SetMember {
                scc_digest: body.scc_digest,
                index: index as usize,
                name,
                kind,
                schema: body.schema.get(),
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
