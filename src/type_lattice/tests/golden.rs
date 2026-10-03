//! The identity recipe, pinned.
//!
//! Two things are fixed here and nowhere else: that every fixed handle is the digest its own node
//! computes, and that no two node kinds share a domain tag. The first catches a recipe change that
//! would silently re-identify a builtin leaf; the second catches a new variant added without one.

use crate::memory::Bump;
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol};

use crate::type_lattice::digest::node_digest;
use crate::type_lattice::handle::{Handle, KType, TypeHandle, builtin_types};
use crate::type_lattice::kind::KKind;
use crate::type_lattice::node::{NodeSchema, TypeNode};
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::render::display_name;
use crate::type_lattice::run::{Elements, Run};
use crate::type_lattice::schema::SigSchema;
use crate::type_lattice::shape::{DeferredReturnSurface, DispatchTokenElement};

#[test]
fn constants_match_freshly_interned_nodes() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let pins: &[(&str, KType, TypeNode<'_>)] = &[
        ("NUMBER", KType::NUMBER, TypeNode::Number),
        ("STR", KType::STR, TypeNode::Str),
        ("BOOL", KType::BOOL, TypeNode::Bool),
        ("NULL", KType::NULL, TypeNode::Null),
        ("IDENTIFIER", KType::IDENTIFIER, TypeNode::Identifier),
        ("SYMBOL", KType::SYMBOL, TypeNode::Symbol),
        (
            "TYPE_NAME_TOKEN",
            KType::TYPE_NAME_TOKEN,
            TypeNode::TypeNameToken,
        ),
        ("EXPRESSION", KType::EXPRESSION, TypeNode::Expression),
        (
            "SIGILED_TYPE_EXPR",
            KType::SIGILED_TYPE_EXPR,
            TypeNode::SigiledTypeExpr,
        ),
        ("RECORD_TYPE", KType::RECORD_TYPE, TypeNode::RecordType),
        ("LITERAL", KType::LITERAL, TypeNode::Literal),
        ("BLOCK", KType::BLOCK, TypeNode::Block),
        ("DECLARATION", KType::DECLARATION, TypeNode::Declaration),
        ("BINDER", KType::BINDER, TypeNode::Binder),
        ("NAME", KType::NAME, TypeNode::Name),
        ("KEYWORD", KType::KEYWORD, TypeNode::Keyword),
        ("ANY", KType::ANY, TypeNode::Any),
        ("ANY_VALUE", KType::ANY_VALUE, TypeNode::AnyValue),
        ("ANY_CODE", KType::ANY_CODE, TypeNode::AnyCode),
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
                element: Handle::ANY,
            },
        ),
        (
            "DICT_ANY_ANY",
            KType::DICT_ANY_ANY,
            TypeNode::Dict {
                key: Handle::ANY,
                value: Handle::ANY,
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
    // The code composites are checked through the doors that mint them, the union's canonical
    // member order being the registry's to choose.
    let type_code = types.union_of(
        region,
        &[
            KType::TYPE_NAME_TOKEN,
            KType::SIGILED_TYPE_EXPR,
            KType::RECORD_TYPE,
        ],
    );
    let composites = [
        ("TYPE_CODE", KType::TYPE_CODE, type_code),
        ("LIST_OF_NAME", KType::LIST_OF_NAME, types.list(KType::NAME)),
        (
            "LIST_OF_DECLARATION",
            KType::LIST_OF_DECLARATION,
            types.list(KType::DECLARATION),
        ),
        (
            "DICT_NAME_BLOCK",
            KType::DICT_NAME_BLOCK,
            types.dict(KType::NAME, KType::BLOCK),
        ),
        (
            "DICT_TYPE_CODE_BLOCK",
            KType::DICT_TYPE_CODE_BLOCK,
            types.dict(type_code, KType::BLOCK),
        ),
        (
            "DICT_NAME_TYPE_CODE",
            KType::DICT_NAME_TYPE_CODE,
            types.dict(KType::NAME, type_code),
        ),
        (
            "QUANTIFIER_CODE",
            KType::QUANTIFIER_CODE,
            types.union_of(region, &[KType::LIST_OF_NAME, KType::DICT_NAME_TYPE_CODE]),
        ),
        (
            "EMPTY_RECORD",
            KType::EMPTY_RECORD,
            types.record(region, &[]),
        ),
    ];
    for (name, pinned, minted) in composites {
        assert_eq!(
            minted, pinned,
            "the pinned `KType::{name}` is not what its door interns",
        );
    }
    // The empty signature carries a schema digest computed at intern time, so it is checked through
    // the intern that mints one rather than against a hand-built node; the signature door answers
    // the constant for an empty draft without interning.
    assert_eq!(
        types.intern_schema(SigSchema::EMPTY),
        Handle::EMPTY_SIGNATURE,
        "the pinned `KType::EMPTY_SIGNATURE` is not what its intern mints for an empty \
         schema",
    );
}

#[test]
fn every_node_kind_has_its_own_tag() {
    let symbols = SymbolInterner::new();
    let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let field = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let keyword = KeywordSymbol::declared("PURE", &symbols).expect("a keyword token");
    // The representatives' runs, declared ahead of the registry so they outlive every node that is
    // interned over them.
    let fields = [(field, KType::NUMBER.raw())];
    let arguments = [(field, KType::STR.raw())];
    let elements = [DispatchTokenElement::Keyword(keyword)];
    let members = [KType::NUMBER.raw(), KType::STR.raw()];
    let needed = [field];
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    // One representative per node kind, whose digests the distinctness assertion below reads. The
    // exhaustive match after the list is what makes a new variant a compile error here.
    let representatives: Vec<TypeNode<'_>> = vec![
        TypeNode::Number,
        TypeNode::Str,
        TypeNode::Bool,
        TypeNode::Null,
        TypeNode::Identifier,
        TypeNode::Symbol,
        TypeNode::TypeNameToken,
        TypeNode::Expression,
        TypeNode::SigiledTypeExpr,
        TypeNode::RecordType,
        TypeNode::Literal,
        TypeNode::Block,
        TypeNode::Declaration,
        TypeNode::Binder,
        TypeNode::Name,
        TypeNode::Keyword,
        TypeNode::Any,
        TypeNode::AnyValue,
        TypeNode::AnyCode,
        TypeNode::Never,
        TypeNode::OfKind(KKind::ProperType),
        TypeNode::Parameter {
            name,
            bound: KType::ANY,
            nonce: None,
        },
        TypeNode::List {
            element: KType::NUMBER.raw(),
        },
        TypeNode::Dict {
            key: KType::NUMBER.raw(),
            value: KType::NUMBER.raw(),
        },
        TypeNode::Record {
            fields: Record::over(&fields),
        },
        TypeNode::KFunction {
            quantifiers: &[],
            bounds: &[],
            params: Record::over(&fields),
            ret: KType::NUMBER.raw(),
        },
        TypeNode::ExpressionShape {
            quantifiers: &[],
            bounds: &[],
            elements: Elements::over(&elements),
            classes: &[],
            ret: KType::NUMBER.raw(),
        },
        TypeNode::Quantified {
            index: 0,
            bound: KType::ANY,
        },
        TypeNode::Lexical {
            level: 0,
            name,
            lower: KType::NEVER,
            bound: KType::ANY,
        },
        TypeNode::Union {
            members: Run::over(&members),
        },
        TypeNode::ConstructorApply {
            constructor: KType::NUMBER.raw(),
            arguments: Record::over(&arguments),
        },
        TypeNode::Signature {
            schema: SigSchema::EMPTY,
            schema_digest: node_digest(region, &TypeNode::Number),
        },
        TypeNode::SignatureApply {
            signature: KType::NUMBER,
            pins: Record::over(&arguments),
        },
        TypeNode::SignatureMeet {
            members: Run::over(&members),
        },
        TypeNode::DeferredReturn(DeferredReturnSurface::Type(name)),
        TypeNode::Sibling(0),
        TypeNode::CodeNeeding {
            kind: KType::EXPRESSION,
            names: &needed,
        },
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
            TypeNode::Symbol => "Symbol",
            TypeNode::TypeNameToken => "TypeNameToken",
            TypeNode::Expression => "Expression",
            TypeNode::SigiledTypeExpr => "SigiledTypeExpr",
            TypeNode::RecordType => "RecordType",
            TypeNode::Literal => "Literal",
            TypeNode::Block => "Block",
            TypeNode::Declaration => "Declaration",
            TypeNode::Binder => "Binder",
            TypeNode::Name => "Name",
            TypeNode::Keyword => "Keyword",
            TypeNode::Any => "Any",
            TypeNode::AnyValue => "AnyValue",
            TypeNode::AnyCode => "AnyCode",
            TypeNode::Never => "Never",
            TypeNode::OfKind(_) => "OfKind",
            TypeNode::Parameter { .. } => "Parameter",
            TypeNode::List { .. } => "List",
            TypeNode::Dict { .. } => "Dict",
            TypeNode::Record { .. } => "Record",
            TypeNode::KFunction { .. } => "KFunction",
            TypeNode::ExpressionShape { .. } => "ExpressionShape",
            TypeNode::Quantified { .. } => "Quantified",
            TypeNode::Lexical { .. } => "Lexical",
            TypeNode::Union { .. } => "Union",
            TypeNode::ConstructorApply { .. } => "ConstructorApply",
            TypeNode::Signature { .. } => "Signature",
            TypeNode::SignatureApply { .. } => "SignatureApply",
            TypeNode::SignatureMeet { .. } => "SignatureMeet",
            TypeNode::DeferredReturn(_) => "DeferredReturn",
            TypeNode::Sibling(_) => "Sibling",
            TypeNode::SetMember { .. } => "SetMember",
            TypeNode::CodeNeeding { .. } => "CodeNeeding",
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
        types.node(handle);
    }
}

/// The family tops' surface spellings: `Value`, `Type` and `Code` each lower to their top, and
/// each top renders back under that name.
#[test]
fn the_family_tops_are_spelled_value_type_and_code() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let types = TypeRegistry::in_region(&bump);
    for (spelling, top) in [
        ("Value", KType::ANY_VALUE),
        ("Type", KType::ANY_TYPE),
        ("Code", KType::ANY_CODE),
    ] {
        let name = TypeSymbol::declared(spelling, &symbols).expect("a Type token");
        assert_eq!(
            KType::from_symbol(name),
            Some(top),
            "`{spelling}` lowers to its top"
        );
        assert_eq!(display_name(top, &types, &symbols).to_string(), spelling);
    }
}

/// Every builtin type name lowers to the handle it names, and every one but the bare containers —
/// which render their parameters — names itself by the same token.
#[test]
fn every_builtin_type_name_round_trips() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let types = TypeRegistry::in_region(&bump);
    for (name, ktype) in builtin_types() {
        let symbol = symbols.record(name);
        assert_eq!(KType::from_symbol(symbol), Some(ktype), "{}", name.text());
        if ktype != KType::LIST_OF_ANY && ktype != KType::DICT_ANY_ANY {
            assert_eq!(
                ktype.name_symbol(&types, &symbols),
                Some(symbol),
                "`{}` names itself",
                name.text()
            );
        }
    }
}
