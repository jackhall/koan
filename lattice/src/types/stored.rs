//! [`StoredNode`] — one interned type as the registry's table stores it.
//!
//! The table stores a node apart from the [`TypeNode`] a reader sees, so storage can pack what a
//! view spells out. Each variant whose probe answers its children decide carries a [`Flags`] byte:
//! the four answers, folded at intern so each probe is one table read. Every other variant's
//! answers are a constant of its kind, which [`StoredNode::flags`] answers.
//!
//! Two conversions connect the two shapes, each one exhaustive match: [`StoredNode::lay`] lays a
//! view as a stored node on an intern miss, and [`StoredNode::view`] reads a stored node back as a
//! view, its children rewrapped as the handle the reader named it by. Every other reader goes
//! through [`TypeRegistry::node`](super::registry::TypeRegistry::node) and never sees this layout.
//!
//! See [README.md](README.md) § The node vocabulary.

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

/// One interned node as the table stores it: [`TypeNode`]'s variants in its order, children raw,
/// and a [`Flags`] byte in each variant whose children decide its probe answers.
#[derive(Clone, Copy)]
pub(super) enum StoredNode<'run> {
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
        name: TypeSymbol,
        key: ContentKey,
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
        classes: &'run [u8],
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
        level: usize,
        name: TypeSymbol,
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
        schema: SigSchema<'run>,
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
        scc_digest: TypeDigest,
        index: usize,
        scc_size: usize,
        name: TypeSymbol,
        kind: KKind,
        schema: NodeSchema<'run>,
    },
}

impl<'run> StoredNode<'run> {
    /// `node` as the table stores it, carrying `flags` where its variant does. Its slices already
    /// live in the region.
    pub(super) fn lay(node: TypeNode<'run>, flags: Flags, _bump: BumpAllocator<'run>) -> Self {
        match node {
            TypeNode::Number => StoredNode::Number,
            TypeNode::Str => StoredNode::Str,
            TypeNode::Bool => StoredNode::Bool,
            TypeNode::Null => StoredNode::Null,
            TypeNode::Identifier => StoredNode::Identifier,
            TypeNode::Symbol => StoredNode::Symbol,
            TypeNode::TypeNameToken => StoredNode::TypeNameToken,
            TypeNode::Expression => StoredNode::Expression,
            TypeNode::SigiledTypeExpr => StoredNode::SigiledTypeExpr,
            TypeNode::RecordType => StoredNode::RecordType,
            TypeNode::Literal => StoredNode::Literal,
            TypeNode::Block => StoredNode::Block,
            TypeNode::Declaration => StoredNode::Declaration,
            TypeNode::Binder => StoredNode::Binder,
            TypeNode::Name => StoredNode::Name,
            TypeNode::Keyword => StoredNode::Keyword,
            TypeNode::Any => StoredNode::Any,
            TypeNode::AnyValue => StoredNode::AnyValue,
            TypeNode::AnyCode => StoredNode::AnyCode,
            TypeNode::CodeNeeding { kind, names } => StoredNode::CodeNeeding { kind, names },
            TypeNode::Never => StoredNode::Never,
            TypeNode::OfKind(kind) => StoredNode::OfKind(kind),
            TypeNode::Parameter { name, bound } => StoredNode::Parameter { name, bound },
            TypeNode::Carrier { name, key, met } => StoredNode::Carrier { name, key, met },
            TypeNode::List { element } => StoredNode::List { flags, element },
            TypeNode::Dict { key, value } => StoredNode::Dict { flags, key, value },
            TypeNode::Record { fields } => StoredNode::Record { flags, fields },
            TypeNode::KFunction { params, ret } => StoredNode::KFunction { flags, params, ret },
            TypeNode::ExpressionShape {
                elements,
                classes,
                ret,
            } => StoredNode::ExpressionShape {
                flags,
                elements,
                classes,
                ret,
            },
            TypeNode::Scheme { group, body } => StoredNode::Scheme { flags, group, body },
            TypeNode::Quantified { index, bound } => StoredNode::Quantified { index, bound },
            TypeNode::Lexical {
                level,
                name,
                lower,
                bound,
            } => StoredNode::Lexical {
                level,
                name,
                lower,
                bound,
            },
            TypeNode::Union { members } => StoredNode::Union { flags, members },
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => StoredNode::ConstructorApply {
                flags,
                constructor,
                arguments,
            },
            TypeNode::Signature {
                schema,
                schema_digest,
            } => StoredNode::Signature {
                schema,
                schema_digest,
            },
            TypeNode::SignatureApply { signature, pins } => StoredNode::SignatureApply {
                flags,
                signature,
                pins,
            },
            TypeNode::SignatureMeet { members } => StoredNode::SignatureMeet { flags, members },
            TypeNode::DeferredReturn(surface) => StoredNode::DeferredReturn(surface),
            TypeNode::Sibling(index) => StoredNode::Sibling(index),
            TypeNode::SetMember {
                scc_digest,
                index,
                scc_size,
                name,
                kind,
                schema,
            } => StoredNode::SetMember {
                scc_digest,
                index,
                scc_size,
                name,
                kind,
                schema,
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
            StoredNode::Number => TypeNode::Number,
            StoredNode::Str => TypeNode::Str,
            StoredNode::Bool => TypeNode::Bool,
            StoredNode::Null => TypeNode::Null,
            StoredNode::Identifier => TypeNode::Identifier,
            StoredNode::Symbol => TypeNode::Symbol,
            StoredNode::TypeNameToken => TypeNode::TypeNameToken,
            StoredNode::Expression => TypeNode::Expression,
            StoredNode::SigiledTypeExpr => TypeNode::SigiledTypeExpr,
            StoredNode::RecordType => TypeNode::RecordType,
            StoredNode::Literal => TypeNode::Literal,
            StoredNode::Block => TypeNode::Block,
            StoredNode::Declaration => TypeNode::Declaration,
            StoredNode::Binder => TypeNode::Binder,
            StoredNode::Name => TypeNode::Name,
            StoredNode::Keyword => TypeNode::Keyword,
            StoredNode::Any => TypeNode::Any,
            StoredNode::AnyValue => TypeNode::AnyValue,
            StoredNode::AnyCode => TypeNode::AnyCode,
            StoredNode::CodeNeeding { kind, names } => TypeNode::CodeNeeding { kind, names },
            StoredNode::Never => TypeNode::Never,
            StoredNode::OfKind(kind) => TypeNode::OfKind(kind),
            StoredNode::Parameter { name, bound } => TypeNode::Parameter { name, bound },
            StoredNode::Carrier { name, key, met } => TypeNode::Carrier { name, key, met },
            StoredNode::List { element, .. } => TypeNode::List {
                element: child(element),
            },
            StoredNode::Dict { key, value, .. } => TypeNode::Dict {
                key: child(key),
                value: child(value),
            },
            StoredNode::Record { fields, .. } => TypeNode::Record {
                fields: Record::over(fields.raw()),
            },
            StoredNode::KFunction { params, ret, .. } => TypeNode::KFunction {
                params: Record::over(params.raw()),
                ret: child(ret),
            },
            StoredNode::ExpressionShape {
                elements,
                classes,
                ret,
                ..
            } => TypeNode::ExpressionShape {
                elements: Elements::over(elements.raw()),
                classes,
                ret: child(ret),
            },
            StoredNode::Scheme { group, body, .. } => TypeNode::Scheme {
                group,
                body: child(body),
            },
            StoredNode::Quantified { index, bound } => TypeNode::Quantified { index, bound },
            StoredNode::Lexical {
                level,
                name,
                lower,
                bound,
            } => TypeNode::Lexical {
                level,
                name,
                lower,
                bound,
            },
            StoredNode::Union { members, .. } => TypeNode::Union {
                members: Run::over(members.raw()),
            },
            StoredNode::ConstructorApply {
                constructor,
                arguments,
                ..
            } => TypeNode::ConstructorApply {
                constructor: child(constructor),
                arguments: Record::over(arguments.raw()),
            },
            StoredNode::Signature {
                schema,
                schema_digest,
            } => TypeNode::Signature {
                schema,
                schema_digest,
            },
            StoredNode::SignatureApply {
                signature, pins, ..
            } => TypeNode::SignatureApply {
                signature,
                pins: Record::over(pins.raw()),
            },
            StoredNode::SignatureMeet { members, .. } => TypeNode::SignatureMeet {
                members: Run::over(members.raw()),
            },
            StoredNode::DeferredReturn(surface) => TypeNode::DeferredReturn(surface),
            StoredNode::Sibling(index) => TypeNode::Sibling(index),
            StoredNode::SetMember {
                scc_digest,
                index,
                scc_size,
                name,
                kind,
                schema,
            } => TypeNode::SetMember {
                scc_digest,
                index,
                scc_size,
                name,
                kind,
                schema,
            },
        }
    }

    /// The node's probe answers — the one door every probe reads. A variant with a [`Flags`] field
    /// answers it; every other variant answers a constant of its kind. A variable's are constant
    /// because its bound and lower end are concrete and carrier-free.
    pub(super) fn flags(&self) -> Flags {
        match *self {
            StoredNode::List { flags, .. }
            | StoredNode::Dict { flags, .. }
            | StoredNode::Record { flags, .. }
            | StoredNode::KFunction { flags, .. }
            | StoredNode::ExpressionShape { flags, .. }
            | StoredNode::Scheme { flags, .. }
            | StoredNode::Union { flags, .. }
            | StoredNode::ConstructorApply { flags, .. }
            | StoredNode::SignatureApply { flags, .. }
            | StoredNode::SignatureMeet { flags, .. } => flags,
            StoredNode::Quantified { .. } => Flags::QUANTIFIED | Flags::RIGID | Flags::PARAMETRIC,
            StoredNode::Lexical { .. } | StoredNode::Parameter { .. } => {
                Flags::RIGID | Flags::PARAMETRIC
            }
            // An opaque carrier: a value carries it and dispatches on it, so it is concrete.
            StoredNode::Carrier { .. } => Flags::CARRIER,
            // Sealed content and leaves: every walk treats them as leaves.
            StoredNode::Number
            | StoredNode::Str
            | StoredNode::Bool
            | StoredNode::Null
            | StoredNode::Identifier
            | StoredNode::Symbol
            | StoredNode::TypeNameToken
            | StoredNode::Expression
            | StoredNode::SigiledTypeExpr
            | StoredNode::RecordType
            | StoredNode::Literal
            | StoredNode::Block
            | StoredNode::Declaration
            | StoredNode::Binder
            | StoredNode::Name
            | StoredNode::Keyword
            | StoredNode::Any
            | StoredNode::AnyValue
            | StoredNode::AnyCode
            | StoredNode::CodeNeeding { .. }
            | StoredNode::Never
            | StoredNode::OfKind(_)
            | StoredNode::Signature { .. }
            | StoredNode::DeferredReturn(_)
            | StoredNode::Sibling(_)
            | StoredNode::SetMember { .. } => Flags::NONE,
        }
    }
}
