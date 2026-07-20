use super::super::node::TypeNode;
use super::super::sig_schema::SigSchema;
use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::TypeRegistry;

// --- Fixed handles ---

/// Ruling 6's guard: every hardcoded handle must be the digest a fresh intern of its own node
/// produces. A digest-recipe change fails here rather than silently re-identifying a leaf, and
/// `TypeRegistry::new` pre-seeds exactly this set so each constant is dereferenceable in a
/// registry that has interned nothing else.
#[test]
fn constants_match_freshly_interned_nodes() {
    let types = TypeRegistry::new();
    let leaves = [
        ("Number", KType::NUMBER, TypeNode::Number),
        ("Str", KType::STR, TypeNode::Str),
        ("Bool", KType::BOOL, TypeNode::Bool),
        ("Null", KType::NULL, TypeNode::Null),
        ("Identifier", KType::IDENTIFIER, TypeNode::Identifier),
        ("KExpression", KType::KEXPRESSION, TypeNode::KExpression),
        (
            "SigiledTypeExpr",
            KType::SIGILED_TYPE_EXPR,
            TypeNode::SigiledTypeExpr,
        ),
        ("RecordType", KType::RECORD_TYPE, TypeNode::RecordType),
        ("Any", KType::ANY, TypeNode::Any),
    ];
    for (label, constant, node) in leaves {
        assert_eq!(types.intern(node), constant, "{label}");
    }
    for kind in [
        KKind::ProperType,
        KKind::Signature,
        KKind::AnyType,
        KKind::NewType,
        KKind::TypeConstructor,
    ] {
        assert_eq!(
            types.intern(TypeNode::OfKind(kind)),
            KType::of_kind(kind),
            "{}",
            kind.surface_keyword()
        );
    }
    assert_eq!(types.list(KType::ANY), KType::LIST_OF_ANY, "List<Any>");
    assert_eq!(
        types.dict(KType::ANY, KType::ANY),
        KType::DICT_ANY_ANY,
        "Dict<Any, Any>"
    );
    assert_eq!(
        types.signature(SigSchema::empty(), Vec::new()),
        KType::EMPTY_SIGNATURE,
        "empty signature"
    );
}

/// Every constant resolves against a registry that has interned nothing of its own, because
/// `TypeRegistry::new` seeds them all.
#[test]
fn constants_resolve_in_a_fresh_registry() {
    let types = TypeRegistry::new();
    for constant in [
        KType::NUMBER,
        KType::STR,
        KType::BOOL,
        KType::NULL,
        KType::IDENTIFIER,
        KType::KEXPRESSION,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::ANY,
        KType::of_kind(KKind::ProperType),
        KType::of_kind(KKind::Signature),
        KType::of_kind(KKind::AnyType),
        KType::of_kind(KKind::NewType),
        KType::of_kind(KKind::TypeConstructor),
        KType::LIST_OF_ANY,
        KType::DICT_ANY_ANY,
        KType::EMPTY_SIGNATURE,
    ] {
        // A miss panics inside `node`.
        let _ = types.node(constant);
    }
}

// --- Rendering ---

#[test]
fn name_renders_parameterized_list() {
    let types = TypeRegistry::new();
    let inner = types.list(KType::NUMBER);
    assert_eq!(
        types.list(inner).name(&types),
        ":(LIST OF :(LIST OF Number))"
    );
}

#[test]
fn name_renders_dict() {
    let types = TypeRegistry::new();
    let t = types.dict(KType::STR, KType::NUMBER);
    assert_eq!(t.name(&types), ":(MAP Str -> Number)");
}

#[test]
fn name_renders_function() {
    let types = TypeRegistry::new();
    let t = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::NUMBER), ("y".into(), KType::STR)]),
        KType::BOOL,
    );
    assert_eq!(t.name(&types), ":(FN (x :Number y :Str) -> Bool)");
}

/// A nested sigiled parameter type already opens with `:`, so the renderer must not prefix a
/// second colon (`xs :(LIST OF Number)`, not `xs ::(LIST OF Number)`).
#[test]
fn name_renders_function_with_sigiled_param() {
    let types = TypeRegistry::new();
    let list_of_number = types.list(KType::NUMBER);
    let t = types.function_type(
        Record::from_pairs(vec![("xs".into(), list_of_number)]),
        KType::NUMBER,
    );
    assert_eq!(t.name(&types), ":(FN (xs :(LIST OF Number)) -> Number)");
}

#[test]
fn name_renders_function_nullary() {
    let types = TypeRegistry::new();
    let t = types.function_type(Record::new(), KType::NULL);
    assert_eq!(t.name(&types), ":(FN () -> Null)");
}

#[test]
fn nominal_kind_surface_keywords() {
    let types = TypeRegistry::new();
    assert_eq!(KType::of_kind(KKind::NewType).name(&types), "NewType");
    assert_eq!(
        KType::of_kind(KKind::TypeConstructor).name(&types),
        "TypeConstructor"
    );
}

/// `:Module` lowers to the empty signature, which renders back as the `Module` surface keyword;
/// `:Signature` is the `OfKind` wildcard and renders as its own keyword.
#[test]
fn any_module_and_any_signature_render_surface_keywords() {
    let types = TypeRegistry::new();
    assert_eq!(KType::EMPTY_SIGNATURE.name(&types), "Module");
    assert_eq!(KType::of_kind(KKind::Signature).name(&types), "Signature");
}

/// A non-empty interface has no declaration label to print, so it renders structurally, in
/// member-name order (ruling 12).
#[test]
fn non_empty_signature_renders_its_members_structurally() {
    let types = TypeRegistry::new();
    let mut schema = SigSchema::empty();
    schema.value_slots.insert("zero".into(), KType::NUMBER);
    schema.value_slots.insert("label".into(), KType::STR);
    let sig = types.signature(schema, Vec::new());
    assert_eq!(sig.name(&types), "SIG (label: Str, zero: Number)");
}

/// `:(A | B)` renders members joined by ` | ` and wrapped in the type sigil.
#[test]
fn name_renders_union() {
    let types = TypeRegistry::new();
    let t = types.union_of(vec![KType::NUMBER, KType::STR]);
    assert_eq!(t.name(&types), ":(Number | Str)");
}

/// A compound member already opens its own sigil, which nests without a doubled colon.
#[test]
fn name_renders_union_with_compound_member() {
    let types = TypeRegistry::new();
    let list_of_number = types.list(KType::NUMBER);
    let t = types.union_of(vec![list_of_number, KType::NULL]);
    assert_eq!(t.name(&types), ":(:(LIST OF Number) | Null)");
}

// --- Identity ---

/// Union identity is order-blind because `union_of` canonicalizes: the same members in a
/// different order intern to one handle.
#[test]
fn union_identity_is_order_blind() {
    let types = TypeRegistry::new();
    let forward = types.union_of(vec![KType::NUMBER, KType::STR]);
    let reversed = types.union_of(vec![KType::STR, KType::NUMBER]);
    assert_eq!(forward, reversed);
}

#[test]
fn unions_of_different_members_are_distinct() {
    let types = TypeRegistry::new();
    let a = types.union_of(vec![KType::NUMBER, KType::STR]);
    let b = types.union_of(vec![KType::NUMBER, KType::BOOL]);
    assert_ne!(a, b);
}

/// `AbstractType` identity keys on its whole content, generativity included: two mints carrying
/// the same nonce and the same member intern to one handle, while a mint nonced against another
/// module — what a second `:|` application of the same SIG produces, since each ascription
/// allocates a fresh child scope — stays distinct. Renaming the member also separates them.
#[test]
fn abstract_type_identity_keys_on_full_content() {
    let types = TypeRegistry::new();
    let source = ScopeId::from_raw(0, 0x51C0);
    let mint = |name: &str, nonce: Option<ScopeId>| {
        types.intern(TypeNode::AbstractType {
            source,
            name: name.into(),
            param_names: Vec::new(),
            nonce,
        })
    };
    let nonce = Some(ScopeId::from_raw(0, 0x0BAB));
    assert_eq!(mint("Carrier", nonce), mint("Carrier", nonce));
    assert_ne!(
        mint("Carrier", nonce),
        mint("Carrier", Some(ScopeId::from_raw(0, 0x0BAC)))
    );
    assert_ne!(mint("Carrier", nonce), mint("Carrier", None));
    assert_ne!(mint("Carrier", nonce), mint("Element", nonce));
}

/// Parameter names are identity but order-blind — they feed the digest sorted, so declaration
/// order is presentation.
#[test]
fn abstract_type_parameter_names_are_an_order_blind_set() {
    let types = TypeRegistry::new();
    let source = ScopeId::from_raw(0, 0x51C0);
    let mint = |params: Vec<&str>| {
        types.intern(TypeNode::AbstractType {
            source,
            name: "Wrap".into(),
            param_names: params.into_iter().map(str::to_string).collect(),
            nonce: None,
        })
    };
    assert_eq!(mint(vec!["Inner", "Outer"]), mint(vec!["Outer", "Inner"]));
    assert_ne!(mint(vec!["Inner"]), mint(vec!["Inner", "Outer"]));
    assert_ne!(mint(vec![]), mint(vec!["Inner"]));
}

/// Function-slot identity is the record substrate's order-blind equality: the same parameters by
/// `(name, type)` in a different declaration order are one type.
#[test]
fn function_params_order_blind_identity() {
    let types = TypeRegistry::new();
    let forward = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::NUMBER), ("y".into(), KType::STR)]),
        KType::BOOL,
    );
    let reversed = types.function_type(
        Record::from_pairs(vec![("y".into(), KType::STR), ("x".into(), KType::NUMBER)]),
        KType::BOOL,
    );
    assert_eq!(forward, reversed);
}

/// Identity is name-sensitive: the same type under a different parameter name is a different
/// function type.
#[test]
fn function_params_name_sensitive_identity() {
    let types = TypeRegistry::new();
    let by_x = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::NUMBER)]),
        KType::BOOL,
    );
    let by_y = types.function_type(
        Record::from_pairs(vec![("y".into(), KType::NUMBER)]),
        KType::BOOL,
    );
    assert_ne!(by_x, by_y);
}

/// A handle is `Copy` — the property every consumer relies on to pass a type by value.
#[test]
fn handle_is_copy() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<KType>();
}

/// `Debug` prints the digest and nothing else: a `Formatter`-only signature has no registry to
/// read content through, and the digest is the whole identity.
#[test]
fn debug_prints_the_digest_in_hex() {
    assert_eq!(
        format!("{:?}", KType::NUMBER),
        "KType(0xe21d67f17aa25f92e072c1bb1f72fc48)"
    );
}
