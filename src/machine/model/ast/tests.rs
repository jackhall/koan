use crate::builtins::test_support::{kw_part, type_name, type_token};
use crate::machine::model::Held;
use crate::machine::model::RunRegistries;
use crate::machine::model::types::KKind;
use crate::machine::model::types::KType;
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::memory::{ProgramBrand, program_storage};
use crate::parse::forms::FormId;
use crate::parse::forms::lazy::LazyKinds;
use crate::parse::{ExpressionPart, KExpression, KLiteral, KeywordSymbol, LabelInterner};
use crate::source::Spanned;

fn kw(s: &str) -> ExpressionPart<'_> {
    kw_part(s)
}

fn ident<'a>(s: &str, labels: &LabelInterner) -> ExpressionPart<'a> {
    ExpressionPart::Identifier(
        crate::parse::ValueSymbol::declared(s, labels)
            .expect("a test fixture identifier is a value token"),
    )
}
fn ty(s: &str) -> ExpressionPart<'_> {
    ExpressionPart::Type(type_token(s))
}
fn num<'a>(n: f64) -> ExpressionPart<'a> {
    ExpressionPart::Literal(KLiteral::Number(n))
}
fn parts_of<'a>(
    items: Vec<ExpressionPart<'a>>,
) -> impl ExactSizeIterator<Item = Spanned<ExpressionPart<'a>>> {
    items.into_iter().map(Spanned::bare)
}
fn expr<'a>(brand: ProgramBrand<'a>, parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    ExpressionPart::expression_from_iter(brand, parts_of(parts))
}
fn list<'a>(brand: ProgramBrand<'a>, items: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    ExpressionPart::ListLiteral(brand.region().allocator().slice_from_iter(items))
}
fn dict<'a>(
    brand: ProgramBrand<'a>,
    pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
) -> ExpressionPart<'a> {
    ExpressionPart::DictLiteral(brand.region().allocator().slice_from_iter(pairs))
}

/// Freeze a run of parts into a node at `brand` — the door every hand-built AST here goes through.
fn build<'a>(brand: ProgramBrand<'a>, items: Vec<ExpressionPart<'a>>) -> KExpression<'a> {
    KExpression::new_from_iter(brand.region(), parts_of(items))
}

#[test]
fn resolve_for_lowers_builtin_leaf_to_type_arm() {
    let storage = crate::memory::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let registries = RunRegistries::new();
    let part = ExpressionPart::Type(type_token("Number"));
    let slot = KType::of_kind(KKind::ProperType);
    // Consume the scope-tied `Held` inside `matches!` so no borrow outlives `storage`.
    assert!(matches!(
        part.resolve_for(&slot, scope, &registries.types),
        Held::Type(t) if t == KType::NUMBER
    ));
}

/// A bare user type name has no builtin lowering, so the bind seam hands it on as the
/// `UnresolvedType` carrier: the token's symbol survives verbatim and no type handle is ever
/// minted for an unresolved name.
#[test]
fn resolve_for_defers_user_bound_leaf_to_unresolved_carrier() {
    let storage = crate::memory::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let registries = RunRegistries::new();
    let part = ExpressionPart::Type(type_name("MyType", &registries));
    let slot = KType::of_kind(KKind::ProperType);
    match part.resolve_for(&slot, scope, &registries.types) {
        Held::UnresolvedType(te) => {
            assert_eq!(registries.labels.render(te.symbol()), "MyType")
        }
        other => panic!(
            "expected the unlowered-name carrier, got {}",
            registries.held_summary(&other)
        ),
    }
}

/// The unlowered carrier still classifies as a proper type for slot matching, so an unresolved
/// name keeps riding the type channel exactly where the lowered arm did.
#[test]
fn unresolved_carrier_classifies_as_a_proper_type() {
    let storage = crate::memory::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let registries = RunRegistries::new();
    let types = &registries.types;
    let part = ExpressionPart::Type(type_token("MyType"));
    let slot = KType::of_kind(KKind::ProperType);
    let held = part.resolve_for(&slot, scope, types);
    assert_eq!(types.ktype_of(&held), KType::of_kind(KKind::ProperType));
    assert!(held.as_type().is_none(), "it carries no type handle");
    assert!(held.as_object().is_none(), "and it is not a value");
}

#[test]
fn summarize_literal_variants() {
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Number(1.5)).summarize(&LabelInterner::new()),
        "1.5"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::String("hi")).summarize(&LabelInterner::new()),
        "hi"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Boolean(true)).summarize(&LabelInterner::new()),
        "true"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Null).summarize(&LabelInterner::new()),
        "null"
    );
}

#[test]
fn summarize_list_and_dict_literals() {
    let program = program_storage();
    let brand = program.brand();
    let items = list(brand, vec![num(1.0), num(2.0)]);
    assert_eq!(items.summarize(&LabelInterner::new()), "[1 2]");

    let pairs = dict(
        brand,
        vec![(ExpressionPart::Literal(KLiteral::String("k")), num(7.0))],
    );
    assert_eq!(pairs.summarize(&LabelInterner::new()), "{k: 7}");
}

#[test]
fn binder_name_from_type_part_extracts_or_none() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let with_type = build(brand, vec![kw("STRUCT"), ty("Point")]);
    assert_eq!(
        with_type.binder_name_from_type_part(),
        Some(type_token("Point"))
    );

    let with_ident = build(brand, vec![kw("STRUCT"), ident("point", &labels)]);
    assert_eq!(with_ident.binder_name_from_type_part(), None);

    let too_short = build(brand, vec![kw("STRUCT")]);
    assert_eq!(too_short.binder_name_from_type_part(), None);
}

#[test]
fn debug_for_expression_part_and_kexpression() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    // Exact format isn't load-bearing; just assert non-empty / tagged output.
    let parts: Vec<ExpressionPart<'_>> = vec![
        kw("LET"),
        ident("x", &labels),
        ty("Number"),
        num(1.0),
        list(brand, vec![ident("a", &labels)]),
        dict(brand, vec![(ident("k", &labels), ident("v", &labels))]),
        expr(brand, vec![ident("z", &labels)]),
    ];
    for p in &parts {
        let s = format!("{:?}", p);
        assert!(!s.is_empty());
    }
    let e = build(brand, parts);
    assert!(format!("{:?}", e).starts_with("KExpression"));
}

/// A union carrier slot captures each part shape through the member that claims it: the raw
/// sigil member rides `Held::Object(KExpression)`, the record member its own `Held::RecordType`,
/// and the two name members ride `Held::Name` carrying the class the parser assigned the token.
#[test]
fn resolve_for_captures_through_a_union_carrier_member() {
    let storage = crate::memory::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let types = &registries.types;
    let slot = types.union_of(&[
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::IDENTIFIER,
    ]);

    let inner = brand.nested_node_from_iter(parts_of(vec![ty("Number")]));
    assert!(matches!(
        ExpressionPart::SigiledTypeExpr(inner).resolve_for(&slot, scope, types),
        Held::Object(crate::machine::model::KObject::KExpression(_)),
    ));
    assert!(matches!(
        ExpressionPart::RecordType(inner).resolve_for(&slot, scope, types),
        Held::RecordType(_),
    ));

    let type_part = ExpressionPart::Type(type_name("Meters", &registries));
    assert!(matches!(
        type_part.resolve_for(&slot, scope, types),
        Held::Name(crate::parse::BinderSymbol::Type(_)),
    ));
    let value_part = ident("width", &registries.labels);
    assert!(matches!(
        value_part.resolve_for(&slot, scope, types),
        Held::Name(crate::parse::BinderSymbol::Value(_)),
    ));
}

/// An `of_kind(…)` member is an ordinary eager member, so a bare `Type` token reaching it is
/// lowered exactly as it is at the bare kind slot — never captured raw as a name.
#[test]
fn resolve_for_lowers_a_type_token_through_a_kind_member() {
    let storage = crate::memory::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let registries = RunRegistries::new();
    let types = &registries.types;
    let slot = types.union_of(&[KType::of_kind(KKind::ProperType), KType::NUMBER]);

    assert!(matches!(
        ExpressionPart::Type(type_token("Number")).resolve_for(&slot, scope, types),
        Held::Type(t) if t == KType::NUMBER
    ));
    assert!(matches!(
        ExpressionPart::Type(type_name("MyType", &registries)).resolve_for(&slot, scope, types),
        Held::UnresolvedType(_)
    ));
}

/// A synthesized run declares nothing, even when the spine the synthesis writes happens to spell a
/// binder form's key. A unary chain reduction emits `<operator> <operands>` — the two-element
/// keyword-led shape `TYPE _` and `NEWTYPE _` also spell — so a chain whose operator is quoted as
/// `TYPE` reduces to a node whose bucket key matches that declaration. The node is not that
/// declaration: it reports no declared-name position, so the park-exemption rail keyed on it does
/// not treat its operand list as a declaration slot.
///
/// The lazy stamp is the other half of the same read and is deliberately *not* withheld: which
/// slots stay raw is a fact about the bucket key, which a synthesized run carries as plainly as a
/// parsed one.
#[test]
fn a_synthesized_run_spelling_a_binder_key_declares_nothing() {
    let program = program_storage();
    let brand = program.brand();
    let region = brand.region();
    let labels = LabelInterner::new();

    let operands = ExpressionPart::ListLiteral(
        region
            .allocator()
            .slice(&[ExpressionPart::Literal(KLiteral::Number(1.0))]),
    );
    let synthesized = WorkingExpression::new(
        region,
        &[
            Spanned::bare(WorkingPart::Ast(ExpressionPart::Keyword(
                KeywordSymbol::declared("TYPE", &labels).expect("TYPE is a keyword token"),
            ))),
            Spanned::bare(WorkingPart::Ast(operands)),
        ],
    );

    assert_eq!(
        synthesized.cache().form().map(|form| form.id),
        Some(FormId::TypeDeclaration),
        "the synthesized key really does match the declaration form",
    );
    assert_eq!(synthesized.binder_name_slot(), None);
    assert!(synthesized.binder_plan().is_none());
    assert_eq!(
        synthesized.lazy_kinds_at(1),
        LazyKinds::CODE,
        "the lazy stamp is a fact about the key and rides every door",
    );
}
