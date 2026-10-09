//! The identity recipe, pinned.
//!
//! Two things are fixed here and nowhere else: that every fixed handle is the digest its own node
//! computes, and that no two node kinds share a domain tag. The first catches a recipe change that
//! would silently re-identify a builtin leaf; the second catches a new variant added without one.

use crate::bump::Bump;
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol, ValueSymbol};

use crate::types::digest::node_digest;
use crate::types::handle::{DeclaredType, Handle, KType, Parametric, TypeHandle, builtin_types};
use crate::types::kind::KKind;
use crate::types::node::{ContentKey, Group, NodeSchema, TypeNode};
use crate::types::record::Record;
use crate::types::registry::TypeRegistry;
use crate::types::render::display_name;
use crate::types::run::{Elements, Run};
use crate::types::schema::{SchemaDraft, SigOrigin, SigSchema};
use crate::types::shape::DispatchTokenElement::{Keyword, Slot};
use crate::types::shape::{DeferredReturnSurface, DispatchTokenElement};
use crate::types::window::{RecursiveGroupWindow, RelativeSchema};

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
            "LIST_OF_EXPRESSION",
            KType::LIST_OF_EXPRESSION,
            types.list(KType::EXPRESSION),
        ),
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
    let (group_names, group_bounds) = ([name], [KType::ANY]);
    let group = Group {
        names: &group_names,
        bounds: &group_bounds,
    };
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let body = types.function_type(region, &fields, KType::NUMBER.raw());
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
        },
        TypeNode::Carrier {
            name,
            key: ContentKey(0),
            met: KType::ANY,
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
            params: Record::over(&fields),
            ret: KType::NUMBER.raw(),
        },
        TypeNode::ExpressionShape {
            elements: Elements::over(&elements),
            classes: &[],
            ret: KType::NUMBER.raw(),
        },
        TypeNode::Scheme {
            group: &group,
            body,
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
    // The exhaustiveness half: `variant_name` matches every variant, so a new one fails to compile
    // there and lands whoever added it in front of the list above.
    for node in &representatives {
        variant_name(node);
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

/// One node of every compound variant, built through the doors, keeps the digest pinned here: a
/// change to how a node is stored may not change what identifies it. Each node also re-digests, as
/// read back, to its own handle.
#[test]
fn every_compound_variant_digests_as_pinned() {
    let symbols = SymbolInterner::new();
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let item = TypeSymbol::declared("Item", &symbols).expect("a Type token");
    let boxed = TypeSymbol::declared("Boxed", &symbols).expect("a Type token");
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let plus = KeywordSymbol::declared("PLUS", &symbols).expect("a keyword token");
    let a = ValueSymbol::declared("a", &symbols).expect("a value token");
    let b = ValueSymbol::declared("b", &symbols).expect("a value token");
    let (region_bump, scratch_bump, host_bump) = (Bump::new(), Bump::new(), Bump::new());
    let (region, scratch, host) = (&region_bump, &scratch_bump, &host_bump);
    let types = TypeRegistry::in_region(region);

    let number_or_str = types.union_of(scratch, &[KType::NUMBER, KType::STR]);
    let variable = types.quantified(0, KType::ANY);
    let parameter = types.head_parameter(elt, KType::ANY);
    let declared = |slot: ValueSymbol, kt: Parametric| {
        let mut draft = SchemaDraft::new(scratch);
        draft.origin = SigOrigin::Declared;
        draft.insert_parameter(elt, parameter);
        draft.insert_value_slot(slot, kt);
        types.signature(scratch, draft)
    };
    let interface = declared(a, parameter);
    let other = declared(b, KType::STR.into());
    let family = {
        let window = RecursiveGroupWindow::new(host, &[(boxed, KKind::TypeConstructor)]);
        let schema = RelativeSchema::constructor(host, scratch, Some(variable), &[elt]);
        window
            .fill_member(0, schema, &types, scratch)
            .and_then(|sealed| sealed.member(0))
            .expect("a singleton window seals on its fill")
    };
    let newtype = {
        let window = RecursiveGroupWindow::new(host, &[(item, KKind::NewType)]);
        window
            .fill_member(0, RelativeSchema::NewType(number_or_str), &types, scratch)
            .and_then(|sealed| sealed.member(0))
            .expect("a singleton window seals on its fill")
    };

    let rows: &[(&str, Handle, u128)] = &[
        (
            "CodeNeeding",
            types
                .code_needing(scratch, KType::EXPRESSION, &[y, x])
                .raw(),
            0xfcfdcdde7c4ee7fea560ecaadc7367a6,
        ),
        (
            "Parameter",
            parameter.raw(),
            0xeab26793ef91a05d163942ddcb9280d1,
        ),
        (
            "Carrier",
            types.carrier(elt, KType::NUMBER, ContentKey(7)).raw(),
            0x9d7d1ef7069d3a83415f765b50f04946,
        ),
        (
            "List",
            types.list(KType::STR).raw(),
            0x5a52215e86782a1c25b2072eb3e5df68,
        ),
        (
            "Dict",
            types.dict(KType::STR, KType::NUMBER).raw(),
            0x0bd3edd7945855a16694c8e61767ab00,
        ),
        (
            "Record",
            types
                .record(scratch, &[(x, KType::NUMBER), (y, KType::STR)])
                .raw(),
            0xe85691a350586d78e53a6839603cc45c,
        ),
        (
            "KFunction",
            types
                .function_type(scratch, &[(x, KType::NUMBER), (y, KType::STR)], KType::BOOL)
                .raw(),
            0x9001f8924ee0b106dca224cebf77a3f8,
        ),
        (
            "ExpressionShape",
            types
                .shape_type(
                    &[Slot(KType::NUMBER), Keyword(plus), Slot(KType::STR)],
                    &[],
                    KType::NUMBER,
                )
                .raw(),
            0x2c58ba607b2aa48111934bda4135802e,
        ),
        (
            "ExpressionShape",
            types
                .shape_type(
                    &[Slot(KType::NUMBER), Keyword(plus), Slot(KType::STR)],
                    &[1, 0],
                    KType::NUMBER,
                )
                .raw(),
            0x02b78ae09baf4e112143c56e0105ac00,
        ),
        (
            "Scheme",
            scheme_handle(
                types
                    .function_scheme(scratch, &[elt], &[KType::ANY], &[(x, variable)], variable)
                    .handle,
            ),
            0x4cd5007f9497d08fab4d856d24eadabb,
        ),
        (
            "Scheme",
            scheme_handle(
                types
                    .shape_scheme(
                        scratch,
                        &[elt],
                        &[KType::NUMBER],
                        &[Keyword(plus), Slot(types.quantified(0, KType::NUMBER))],
                        &[],
                        types.quantified(0, KType::NUMBER),
                    )
                    .handle,
            ),
            0x9b7d42acb416fa2c3b5899f9df6971c1,
        ),
        (
            "Quantified",
            variable.raw(),
            0x2af8c8d9a319436e3995725b1d5afc2d,
        ),
        (
            "Lexical",
            types.lexical(3, elt, KType::ANY).raw(),
            0xf164da60717456d28cf894cc991cb2e7,
        ),
        (
            "Lexical",
            types
                .lexical_between(scratch, 3, elt, KType::NUMBER, number_or_str)
                .raw(),
            0xfed70962599c6d653afa4918b3b0b177,
        ),
        (
            "Union",
            number_or_str.raw(),
            0x53af01e77c424fc800f36edc0462d536,
        ),
        (
            "ConstructorApply",
            types
                .constructor_apply(scratch, family, &[(BinderSymbol::Type(elt), KType::NUMBER)])
                .raw(),
            0x0532180519c9dd589a3216e0de102d8f,
        ),
        (
            "Signature",
            interface.raw(),
            0x71a19019b5dddb550cf6aa7f322066db,
        ),
        (
            "SignatureApply",
            types
                .signature_apply(scratch, interface, &[(BinderSymbol::Type(elt), KType::STR)])
                .raw(),
            0x6eae66cc7abbfd4d4f1dd5d792fef5b2,
        ),
        (
            "SignatureMeet",
            types.signature_meet(scratch, &[interface, other]).raw(),
            0x3ee40c0b74a2172dac6e5c3a6f00bf29,
        ),
        (
            "DeferredReturn",
            types
                .deferred_return(DeferredReturnSurface::Type(elt))
                .raw(),
            0x6445be5d5582fa0f37bd0d40c6d50dfd,
        ),
        (
            "DeferredReturn",
            types
                .deferred_return(DeferredReturnSurface::Expression("x + 1"))
                .raw(),
            0x4ec79454c2e2b295abb93d9c6b5fa542,
        ),
        (
            "Sibling",
            types.sibling(0).raw(),
            0xa9ed2eded73872690bfd24478962b67d,
        ),
        (
            "SetMember",
            newtype.raw(),
            0x7b3a3bf25eba5ef8341f9c3d0328c38c,
        ),
        (
            "SetMember",
            family.raw(),
            0x0dd73ff448568a2a11775f24b3940ac0,
        ),
    ];
    let mut covered = std::collections::BTreeSet::new();
    for (label, handle, pinned) in rows {
        let node = types.node(*handle);
        assert_eq!(
            variant_name(&node),
            *label,
            "the row builds the variant it names"
        );
        assert_eq!(
            node_digest(scratch, &node),
            handle.digest(),
            "a `{label}` node read back re-digests to its own handle",
        );
        assert_eq!(
            handle.digest().0,
            *pinned,
            "a `{label}` node's digest moved"
        );
        covered.insert(*label);
    }
    let compound = [
        "CodeNeeding",
        "Parameter",
        "Carrier",
        "List",
        "Dict",
        "Record",
        "KFunction",
        "ExpressionShape",
        "Scheme",
        "Quantified",
        "Lexical",
        "Union",
        "ConstructorApply",
        "Signature",
        "SignatureApply",
        "SignatureMeet",
        "DeferredReturn",
        "Sibling",
        "SetMember",
    ];
    assert_eq!(covered, compound.into_iter().collect());
}

/// A scheme door's declared type, which every door above answers a scheme for.
fn scheme_handle(declared: DeclaredType<Parametric>) -> Handle {
    match declared {
        DeclaredType::Scheme(scheme) => scheme.raw(),
        DeclaredType::Type(_) => panic!("a non-empty group binds a scheme"),
    }
}

/// The name of `node`'s variant: one exhaustive match, so a new variant is a compile error here.
fn variant_name<H>(node: &TypeNode<'_, H>) -> &'static str {
    match node {
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
        TypeNode::Carrier { .. } => "Carrier",
        TypeNode::List { .. } => "List",
        TypeNode::Dict { .. } => "Dict",
        TypeNode::Record { .. } => "Record",
        TypeNode::KFunction { .. } => "KFunction",
        TypeNode::ExpressionShape { .. } => "ExpressionShape",
        TypeNode::Scheme { .. } => "Scheme",
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
