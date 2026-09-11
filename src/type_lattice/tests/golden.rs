//! The identity recipe, pinned.
//!
//! Two things are fixed here and nowhere else: that every fixed handle is the digest its own node
//! computes, and that no two node kinds share a domain tag. The first catches a recipe change that
//! would silently re-identify a builtin leaf; the second catches a new variant added without one.

use crate::memory::ScopeId;
use crate::parse::{BinderSymbol, KeywordSymbol, LabelInterner, TypeSymbol};

use crate::type_lattice::digest::node_digest;
use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::node::{NodeSchema, TypeNode};
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{SchemaDraft, SigSchema};
use crate::type_lattice::shape::{DeferredReturnSurface, DispatchTokenElement};

use super::generators::{allocator, fresh_cart};

#[test]
fn constants_match_freshly_interned_nodes() {
    let cart = fresh_cart();
    let region = allocator(&cart);
    let types = TypeRegistry::in_region(region);
    let pins: &[(&str, KType, TypeNode<'_>)] = &[
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
            node_digest(region, node),
            pinned.digest(),
            "the pinned `KType::{name}` is not the digest its own node computes",
        );
    }
    // The empty signature carries a schema digest computed at intern time, so it is checked through
    // the door that mints one rather than against a hand-built node.
    assert_eq!(
        types.signature(region, SchemaDraft::new(region)),
        KType::EMPTY_SIGNATURE,
        "the pinned `KType::EMPTY_SIGNATURE` is not what the signature door interns for an empty \
         schema",
    );
}

#[test]
fn every_node_kind_has_its_own_tag() {
    let labels = LabelInterner::new();
    let name = TypeSymbol::declared("Elt", &labels).expect("a Type token");
    let field = BinderSymbol::declared("x", &labels).expect("a bindable token");
    let keyword = KeywordSymbol::declared("PURE", &labels).expect("a keyword token");
    // The representatives' runs, declared ahead of the registry so they outlive every node that is
    // interned over them.
    let fields = [(field, KType::NUMBER)];
    let arguments = [(field, KType::STR)];
    let elements = [DispatchTokenElement::Keyword(keyword)];
    let members = [KType::NUMBER, KType::STR];
    let cart = fresh_cart();
    let region = allocator(&cart);
    let types = TypeRegistry::in_region(region);
    // One representative per node kind, whose digests the distinctness assertion below reads. The
    // exhaustive match after the list is what makes a new variant a compile error here.
    let representatives: Vec<TypeNode<'_>> = vec![
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
            source: ScopeId::SENTINEL,
            name,
            param_names: &[],
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
            fields: Record::over(&fields),
        },
        TypeNode::KFunction {
            params: Record::over(&fields),
            ret: KType::NUMBER,
        },
        TypeNode::ExpressionShape {
            quantifiers: &[],
            bounds: &[],
            elements: &elements,
            ret: KType::NUMBER,
        },
        TypeNode::Quantified {
            index: 0,
            bound: KType::ANY,
        },
        TypeNode::Union { members: &members },
        TypeNode::ConstructorApply {
            constructor: KType::NUMBER,
            arguments: Record::over(&arguments),
        },
        TypeNode::Signature {
            schema: SigSchema::EMPTY,
            schema_digest: node_digest(region, &TypeNode::Number),
        },
        TypeNode::DeferredReturn(DeferredReturnSurface::Type(name)),
        TypeNode::Sibling(0),
        TypeNode::SetMember {
            scc_digest: node_digest(region, &TypeNode::Number),
            index: 0,
            scc_size: 1,
            name,
            kind: KKind::NewType,
            schema: NodeSchema::NewType(KType::NUMBER),
        },
    ];
    // The exhaustiveness half: a match naming every variant, so a new one fails to compile here
    // and lands whoever added it in front of the list above.
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
    let digests: Vec<_> = representatives
        .iter()
        .map(|node| node_digest(region, node))
        .collect();
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
        let handle = types.intern(region, node);
        types.with_node(handle, |_| ());
    }
}
