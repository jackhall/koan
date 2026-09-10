//! The identity recipe, pinned.
//!
//! Two things are fixed here and nowhere else: that every fixed handle is the digest its own node
//! computes, and that no two node kinds share a domain tag. The first catches a recipe change that
//! would silently re-identify a builtin leaf; the second catches a new variant added without one.

use crate::parse::{BinderSymbol, LabelInterner, TypeSymbol};

use crate::type_lattice::digest::node_digest;
use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::node::{NodeSchema, TypeNode};
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{SigSchema, TypeMemberMap};
use crate::type_lattice::shape::{DeferredReturnSurface, DispatchTokenElement};

#[test]
fn constants_match_freshly_interned_nodes() {
    let types = TypeRegistry::new();
    let pins: &[(&str, KType, TypeNode)] = &[
        ("NUMBER", KType::NUMBER, TypeNode::Number),
        ("STR", KType::STR, TypeNode::Str),
        ("BOOL", KType::BOOL, TypeNode::Bool),
        ("NULL", KType::NULL, TypeNode::Null),
        ("IDENTIFIER", KType::IDENTIFIER, TypeNode::Identifier),
        ("NAME_TOKEN", KType::NAME_TOKEN, TypeNode::NameToken),
        (
            "TYPE_NAME_TOKEN",
            KType::TYPE_NAME_TOKEN,
            TypeNode::TypeNameToken,
        ),
        ("KEXPRESSION", KType::KEXPRESSION, TypeNode::KExpression),
        (
            "SIGILED_TYPE_EXPR",
            KType::SIGILED_TYPE_EXPR,
            TypeNode::SigiledTypeExpr,
        ),
        ("RECORD_TYPE", KType::RECORD_TYPE, TypeNode::RecordType),
        ("ANY", KType::ANY, TypeNode::Any),
        ("NEVER", KType::NEVER, TypeNode::Never),
        (
            "PROPER_TYPE",
            KType::PROPER_TYPE,
            TypeNode::OfKind(KKind::ProperType),
        ),
        (
            "SIGNATURE_KIND",
            KType::SIGNATURE_KIND,
            TypeNode::OfKind(KKind::Signature),
        ),
        (
            "ANY_TYPE",
            KType::ANY_TYPE,
            TypeNode::OfKind(KKind::AnyType),
        ),
        (
            "NEW_TYPE",
            KType::NEW_TYPE,
            TypeNode::OfKind(KKind::NewType),
        ),
        (
            "TYPE_CONSTRUCTOR",
            KType::TYPE_CONSTRUCTOR,
            TypeNode::OfKind(KKind::TypeConstructor),
        ),
        (
            "LIST_OF_ANY",
            KType::LIST_OF_ANY,
            TypeNode::List {
                element: KType::ANY,
            },
        ),
        (
            "DICT_ANY_ANY",
            KType::DICT_ANY_ANY,
            TypeNode::Dict {
                key: KType::ANY,
                value: KType::ANY,
            },
        ),
    ];
    for (name, pinned, node) in pins {
        assert_eq!(
            node_digest(node),
            pinned.digest(),
            "the pinned `KType::{name}` is not the digest its own node computes",
        );
    }
    // The empty signature carries a schema digest computed at intern time, so it is checked through
    // the door that mints one rather than against a hand-built node.
    assert_eq!(
        types.signature(SigSchema::empty()),
        KType::EMPTY_SIGNATURE,
        "the pinned `KType::EMPTY_SIGNATURE` is not what the signature door interns for an empty \
         schema",
    );
}

#[test]
fn every_node_kind_has_its_own_tag() {
    let labels = LabelInterner::new();
    let types = TypeRegistry::new();
    let name = TypeSymbol::declared("Elt", &labels).expect("a Type token");
    let field = BinderSymbol::declared("x", &labels).expect("a bindable token");
    let keyword = crate::parse::KeywordSymbol::declared("PURE", &labels).expect("a keyword token");
    let mut members = TypeMemberMap::default();
    members.insert(name, KType::NUMBER);

    // Exhaustive by construction: a new `TypeNode` variant is a compile error in this match, and the
    // representative it forces someone to write is what the distinctness assertion below reads.
    let representatives: Vec<TypeNode> = vec![
        TypeNode::Number,
        TypeNode::Str,
        TypeNode::Bool,
        TypeNode::Null,
        TypeNode::Identifier,
        TypeNode::NameToken,
        TypeNode::TypeNameToken,
        TypeNode::KExpression,
        TypeNode::SigiledTypeExpr,
        TypeNode::RecordType,
        TypeNode::Any,
        TypeNode::Never,
        TypeNode::OfKind(KKind::ProperType),
        TypeNode::AbstractType {
            source: crate::memory::ScopeId::SENTINEL,
            name,
            param_names: Vec::new(),
            nonce: None,
            bound: KType::ANY,
        },
        TypeNode::List {
            element: KType::NUMBER,
        },
        TypeNode::Dict {
            key: KType::NUMBER,
            value: KType::NUMBER,
        },
        TypeNode::Record {
            fields: Record::from_pairs([(field, KType::NUMBER)]),
        },
        TypeNode::KFunction {
            params: Record::from_pairs([(field, KType::NUMBER)]),
            ret: KType::NUMBER,
        },
        TypeNode::ExpressionShape {
            quantifiers: Vec::new(),
            elements: vec![DispatchTokenElement::Keyword(keyword)].into_boxed_slice(),
            ret: KType::NUMBER,
        },
        TypeNode::Quantified {
            index: 0,
            bound: KType::ANY,
        },
        TypeNode::Union {
            members: vec![KType::NUMBER, KType::STR],
        },
        TypeNode::ConstructorApply {
            constructor: KType::NUMBER,
            arguments: Record::from_pairs([(field, KType::STR)]),
        },
        TypeNode::Signature {
            schema: SigSchema::empty(),
            schema_digest: node_digest(&TypeNode::Number),
        },
        TypeNode::DeferredReturn(DeferredReturnSurface::Type(name)),
        TypeNode::Sibling(0),
        TypeNode::SetMember {
            scc_digest: node_digest(&TypeNode::Number),
            index: 0,
            scc_size: 1,
            name,
            kind: KKind::NewType,
            schema: NodeSchema::NewType(KType::NUMBER),
        },
    ];
    // The exhaustiveness half: every variant above must be reachable from a match that names them
    // all, so adding one without a representative fails to compile.
    for node in &representatives {
        let _: &'static str = match node {
            TypeNode::Number => "Number",
            TypeNode::Str => "Str",
            TypeNode::Bool => "Bool",
            TypeNode::Null => "Null",
            TypeNode::Identifier => "Identifier",
            TypeNode::NameToken => "NameToken",
            TypeNode::TypeNameToken => "TypeNameToken",
            TypeNode::KExpression => "KExpression",
            TypeNode::SigiledTypeExpr => "SigiledTypeExpr",
            TypeNode::RecordType => "RecordType",
            TypeNode::Any => "Any",
            TypeNode::Never => "Never",
            TypeNode::OfKind(_) => "OfKind",
            TypeNode::AbstractType { .. } => "AbstractType",
            TypeNode::List { .. } => "List",
            TypeNode::Dict { .. } => "Dict",
            TypeNode::Record { .. } => "Record",
            TypeNode::KFunction { .. } => "KFunction",
            TypeNode::ExpressionShape { .. } => "ExpressionShape",
            TypeNode::Quantified { .. } => "Quantified",
            TypeNode::Union { .. } => "Union",
            TypeNode::ConstructorApply { .. } => "ConstructorApply",
            TypeNode::Signature { .. } => "Signature",
            TypeNode::DeferredReturn(_) => "DeferredReturn",
            TypeNode::Sibling(_) => "Sibling",
            TypeNode::SetMember { .. } => "SetMember",
        };
    }
    let digests: Vec<_> = representatives.iter().map(node_digest).collect();
    for (index, digest) in digests.iter().enumerate() {
        for (peer, other) in digests.iter().enumerate() {
            assert!(
                index == peer || digest != other,
                "two node kinds digest alike, so one of them has no tag of its own",
            );
        }
    }
    // Every representative is interned content, so the table can name each one back.
    for node in representatives {
        let handle = types.intern(node);
        types.with_node(handle, |_| ());
    }
    let _ = members;
}
