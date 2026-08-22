use std::collections::HashMap;
use std::rc::Rc;

use crate::builtins::test_support::kw_part;
use crate::builtins::test_support::type_name;
use crate::builtins::test_support::type_token;
use crate::machine::core::program_storage;
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeMemberMap;
use crate::machine::model::TypeRegistry;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::types::{KKind, KType, Record, RecursiveGroupWindow, RelativeSchema};
use crate::machine::model::values::{Held, KKey, KObject, ValueEqualityError};
use crate::source::Spanned;

fn num<'a>(n: f64) -> KObject<'a> {
    KObject::Number(n)
}

fn part<'a>(p: ExpressionPart<'a>) -> Spanned<ExpressionPart<'a>> {
    Spanned::bare(p)
}

fn newtype_singleton(name: &str, repr: KType, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        type_token(name),
        RelativeSchema::NewType(repr),
        None,
        types,
    )
}

/// Mint the zero-dep fold door a container test needs, over a fresh root region, as two `let`
/// bindings in the caller's own scope: `forge_for_test` is the sanctioned test-only placement
/// mint (no enclosing fold engine required). A statement macro (not a function returning the
/// pair) so `door`'s borrow of `storage` lives in the same frame it was minted in, never crossing
/// a return.
macro_rules! container_door {
    ($storage:ident, $door:ident) => {
        use crate::machine::core::{FoldingBrand, FrameStorageExt, run_root_storage};
        use crate::witnessed::FoldedPlacement;
        let $storage = run_root_storage();
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let $door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            $storage.brand().handle(),
        ))
        .with_holder(&owned_cells);
    };
}

// --- scalars ----------------------------------------------------------------------

#[test]
fn number_ieee_semantics() {
    let registries = RunRegistries::new();
    assert_eq!(num(1.0).value_equal(&num(1.0), &registries), Ok(true));
    assert_eq!(num(1.0).value_equal(&num(2.0), &registries), Ok(false));
    // NaN is equal to nothing, including itself.
    assert_eq!(
        num(f64::NAN).value_equal(&num(f64::NAN), &registries),
        Ok(false)
    );
    // Signed zeros compare equal.
    assert_eq!(num(-0.0).value_equal(&num(0.0), &registries), Ok(true));
}

#[test]
fn string_bool_null_scalars() {
    let registries = RunRegistries::new();
    let s = KObject::KString("a");
    assert_eq!(s.value_equal(&KObject::KString("a"), &registries), Ok(true));
    assert_eq!(
        s.value_equal(&KObject::KString("b"), &registries),
        Ok(false)
    );
    assert_eq!(
        KObject::Bool(true).value_equal(&KObject::Bool(true), &registries),
        Ok(true)
    );
    assert_eq!(
        KObject::Bool(true).value_equal(&KObject::Bool(false), &registries),
        Ok(false)
    );
    assert_eq!(
        KObject::Null.value_equal(&KObject::Null, &registries),
        Ok(true)
    );
}

#[test]
fn cross_variant_scalars_are_unequal() {
    let registries = RunRegistries::new();
    assert_eq!(
        num(1.0).value_equal(&KObject::KString("a"), &registries),
        Ok(false)
    );
    assert_eq!(KObject::Null.value_equal(&num(0.0), &registries), Ok(false));
    assert_eq!(
        KObject::Bool(true).value_equal(&KObject::KString("true"), &registries),
        Ok(false)
    );
}

// --- lists ------------------------------------------------------------------------

#[test]
fn list_element_and_length() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let a = KObject::list(door, vec![num(1.0), num(2.0)], types);
    let b = KObject::list(door, vec![num(1.0), num(2.0)], types);
    let c = KObject::list(door, vec![num(1.0), num(3.0)], types);
    let short = KObject::list(door, vec![num(1.0)], types);
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
    assert_eq!(a.value_equal(&c, &registries), Ok(false));
    assert_eq!(a.value_equal(&short, &registries), Ok(false));
}

#[test]
fn list_nan_self_compare_is_false() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    // No Rc-ptr fast path: a self-comparison of a NaN-holding list is element-wise false.
    let l = KObject::list(door, vec![num(f64::NAN)], types);
    assert_eq!(l.value_equal(&l, &registries), Ok(false));
}

#[test]
fn list_comparability_gate_is_intransitive() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    // `[]:Number` == `[]:Any` == `[]:Str`, but the outer two are unrelated → unequal.
    let empty_number =
        KObject::list(door, vec![], types).stamp_type(types.list(KType::NUMBER), types);
    let empty_any = KObject::list(door, vec![], types).stamp_type(types.list(KType::ANY), types);
    let empty_str = KObject::list(door, vec![], types).stamp_type(types.list(KType::STR), types);
    assert_eq!(empty_number.value_equal(&empty_any, &registries), Ok(true));
    assert_eq!(empty_any.value_equal(&empty_str, &registries), Ok(true));
    // Number and Str are unrelated → gate closes, no descent.
    assert_eq!(empty_number.value_equal(&empty_str, &registries), Ok(false));
}

#[test]
fn list_of_types_compares_by_digest() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let a = KObject::list_of_held(door, &[Held::Type(KType::NUMBER)], types);
    let b = KObject::list_of_held(door, &[Held::Type(KType::NUMBER)], types);
    let c = KObject::list_of_held(door, &[Held::Type(KType::STR)], types);
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
    // Different element type parameters (a `Type OF Number` vs `Type OF Str` list) close the gate.
    assert_eq!(a.value_equal(&c, &registries), Ok(false));
}

// --- dicts ------------------------------------------------------------------------

fn dict<'a>(
    door: crate::machine::core::SubstrateDoor<'a, '_>,
    pairs: Vec<(KKey, KObject<'a>)>,
    types: &TypeRegistry,
) -> KObject<'a> {
    let mut map: HashMap<KKey, KObject<'a>> = HashMap::new();
    for (k, v) in pairs {
        map.insert(k, v);
    }
    KObject::dict(door, map, types)
}

#[test]
fn dict_key_and_value_equality() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let a = dict(
        door,
        vec![(KKey::String("x"), num(1.0)), (KKey::String("y"), num(2.0))],
        types,
    );
    let b = dict(
        door,
        vec![(KKey::String("y"), num(2.0)), (KKey::String("x"), num(1.0))],
        types,
    );
    assert_eq!(a.value_equal(&b, &registries), Ok(true));

    let missing_key = dict(
        door,
        vec![(KKey::String("x"), num(1.0)), (KKey::String("z"), num(2.0))],
        types,
    );
    assert_eq!(a.value_equal(&missing_key, &registries), Ok(false));

    let diff_value = dict(
        door,
        vec![(KKey::String("x"), num(1.0)), (KKey::String("y"), num(9.0))],
        types,
    );
    assert_eq!(a.value_equal(&diff_value, &registries), Ok(false));
}

#[test]
fn dict_length_mismatch_is_false() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let a = dict(door, vec![(KKey::String("x"), num(1.0))], types);
    let b = dict(
        door,
        vec![(KKey::String("x"), num(1.0)), (KKey::String("y"), num(2.0))],
        types,
    );
    assert_eq!(a.value_equal(&b, &registries), Ok(false));
}

// --- records ----------------------------------------------------------------------

fn record<'a>(
    door: crate::machine::core::SubstrateDoor<'a, '_>,
    pairs: Vec<(&str, KObject<'a>)>,
    types: &TypeRegistry,
) -> KObject<'a> {
    let fields: Vec<_> = pairs
        .into_iter()
        .map(|(k, v)| (crate::machine::model::Symbol::of(k), v))
        .collect();
    KObject::record(door, &fields, types)
}

#[test]
fn record_field_order_blind_equality() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let a = record(door, vec![("x", num(1.0)), ("y", num(2.0))], types);
    let b = record(door, vec![("y", num(2.0)), ("x", num(1.0))], types);
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
}

#[test]
fn record_width_mismatch_comparable_but_unequal() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    // `{x:Number}` and `{x:Number, y:Number}` are related by record subtyping (gate open),
    // but the field sets differ → unequal.
    let narrow = record(door, vec![("x", num(1.0))], types);
    let wide = record(door, vec![("x", num(1.0)), ("y", num(2.0))], types);
    assert_eq!(narrow.value_equal(&wide, &registries), Ok(false));
}

#[test]
fn record_field_value_differs() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let a = record(door, vec![("x", num(1.0))], types);
    let b = record(door, vec![("x", num(2.0))], types);
    assert_eq!(a.value_equal(&b, &registries), Ok(false));
}

// --- tagged -----------------------------------------------------------------------

/// Two singleton newtype members declared together, so distinct handles exist for the
/// identity check. Returns the `None`-over-`Null` and `Some`-over-`Number` member handles.
fn two_member(types: &TypeRegistry) -> Vec<KType> {
    let window = RecursiveGroupWindow::new(vec![
        (type_token("None"), KKind::NewType),
        (type_token("Some"), KKind::NewType),
    ]);
    window.fill_member(0, RelativeSchema::NewType(KType::NULL), types);
    window
        .fill_member(1, RelativeSchema::NewType(KType::NUMBER), types)
        .expect("the last fill seals a fully declared window")
        .members
}

#[test]
fn tagged_same_nominal_compares_payload() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let identity = newtype_singleton("Distance", KType::NUMBER, types);
    let a = KObject::tagged(door, type_token("Distance"), &num(3.0), identity);
    let b = KObject::tagged(door, type_token("Distance"), &num(3.0), identity);
    let c = KObject::tagged(door, type_token("Distance"), &num(4.0), identity);
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
    assert_eq!(a.value_equal(&c, &registries), Ok(false));
}

/// Identity-based equality reads an erased carrier (the bare member handle) and a stamped one
/// (a `ConstructorApply` over that member) as distinct types, so they compare unequal even with
/// equal payloads — the erased-vs-stamped distinction lives in the one identity handle.
#[test]
fn tagged_erased_and_stamped_are_distinct_identities() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let ctor = RecursiveGroupWindow::seal_singleton(
        type_token("Box"),
        RelativeSchema::TypeConstructor {
            schema: TypeMemberMap::default(),
            param_names: vec![type_name("Type", &registries)],
        },
        None,
        types,
    );
    let erased = KObject::tagged(door, type_token("Box"), &num(1.0), ctor);
    let stamped = KObject::tagged(
        door,
        type_token("Box"),
        &num(1.0),
        types.constructor_apply(
            ctor,
            Record::from_pairs([(crate::machine::model::Symbol::of("Type"), KType::NUMBER)]),
        ),
    );
    assert_eq!(erased.value_equal(&stamped, &registries), Ok(false));
}

#[test]
fn tagged_distinct_index_is_unequal() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let members = two_member(types);
    let none = KObject::tagged(door, type_token("None"), &KObject::Null, members[0]);
    let some = KObject::tagged(door, type_token("Some"), &num(1.0), members[1]);
    assert_eq!(none.value_equal(&some, &registries), Ok(false));
}

// --- wrapped ----------------------------------------------------------------------

#[test]
fn wrapped_identity_and_payload() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let type_id = newtype_singleton("Distance", KType::NUMBER, types);
    let a = KObject::wrapped_hold(door, &num(3.0), type_id);
    let b = KObject::wrapped_hold(door, &num(3.0), type_id);
    let diff_payload = KObject::wrapped_hold(door, &num(4.0), type_id);
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
    assert_eq!(a.value_equal(&diff_payload, &registries), Ok(false));
    // A wrapped value is never equal to its bare representation.
    assert_eq!(a.value_equal(&num(3.0), &registries), Ok(false));
}

#[test]
fn wrapped_distinct_nominal_is_unequal() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let distance = newtype_singleton("Distance", KType::NUMBER, types);
    let weight = newtype_singleton("Weight", KType::NUMBER, types);
    let a = KObject::wrapped_hold(door, &num(3.0), distance);
    let b = KObject::wrapped_hold(door, &num(3.0), weight);
    assert_eq!(a.value_equal(&b, &registries), Ok(false));
}

// --- expressions ------------------------------------------------------------------

#[test]
fn kexpression_structural_equality() {
    let registries = RunRegistries::new();
    let program = program_storage();
    let brand = program.brand();
    let a = KObject::KExpression(
        brand.new_expression(&[part(kw_part("LET")), part(ExpressionPart::Identifier("x"))]),
    );
    let b = KObject::KExpression(
        brand.new_expression(&[part(kw_part("LET")), part(ExpressionPart::Identifier("x"))]),
    );
    let c = KObject::KExpression(
        brand.new_expression(&[part(kw_part("LET")), part(ExpressionPart::Identifier("y"))]),
    );
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
    assert_eq!(a.value_equal(&c, &registries), Ok(false));
}

#[test]
fn kexpression_number_literal_is_ieee() {
    let registries = RunRegistries::new();
    let program = program_storage();
    let brand = program.brand();
    let nan = KObject::KExpression(
        brand.new_expression(&[part(ExpressionPart::Literal(KLiteral::Number(f64::NAN)))]),
    );
    assert_eq!(nan.value_equal(&nan, &registries), Ok(false));
    let one = KObject::KExpression(
        brand.new_expression(&[part(ExpressionPart::Literal(KLiteral::Number(1.0)))]),
    );
    let one2 = KObject::KExpression(
        brand.new_expression(&[part(ExpressionPart::Literal(KLiteral::Number(1.0)))]),
    );
    assert_eq!(one.value_equal(&one2, &registries), Ok(true));
}

#[test]
fn kexpression_length_and_variant_mismatch() {
    let registries = RunRegistries::new();
    let program = program_storage();
    let brand = program.brand();
    let a = KObject::KExpression(brand.new_expression(&[part(kw_part("LET"))]));
    let longer = KObject::KExpression(
        brand.new_expression(&[part(kw_part("LET")), part(ExpressionPart::Identifier("x"))]),
    );
    // Different part variants at the same position.
    let variant =
        KObject::KExpression(brand.new_expression(&[part(ExpressionPart::Identifier("LET"))]));
    assert_eq!(a.value_equal(&longer, &registries), Ok(false));
    assert_eq!(a.value_equal(&variant, &registries), Ok(false));
}

// --- banned operands --------------------------------------------------------------

/// A function value allocated in `storage`, closing over `scope` — the run root's own scope, so
/// the value is the one a real run would build.
fn a_function<'a>(
    storage: &'a Rc<crate::machine::core::FrameStorage>,
    scope: &'a crate::machine::Scope<'a>,
    registries: &RunRegistries,
) -> KObject<'a> {
    use crate::machine::KFunction;
    use crate::machine::core::{Body, FrameStorageExt};
    use crate::machine::model::types::{ReturnType, SignatureDraft};
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::NUMBER),
        elements: Vec::new(),
    };
    let f = KFunction::alloc_captured_for_test(
        scope,
        sig,
        Body::UserDefined(KExpression::new(storage.brand(), &[])),
        registries,
    );
    KObject::KFunction(f)
}

#[test]
fn function_operand_is_error_at_any_position() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;
    let program = program_storage();
    let storage = run_root_storage();
    let test_run = TestRun::silent(&program, &storage);
    let types = test_run.registry_handle();
    let f = a_function(&storage, test_run.scope, types.registries());
    assert_eq!(
        f.value_equal(&num(1.0), types.registries()),
        Err(ValueEqualityError::Function)
    );
    assert_eq!(
        num(1.0).value_equal(&f, types.registries()),
        Err(ValueEqualityError::Function)
    );
    // Nested: a function inside a list propagates the error.
    let program2 = program_storage();
    let storage2 = run_root_storage();
    let second_run = TestRun::silent(&program2, &storage2);
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door = {
        use crate::machine::core::{FoldingBrand, FrameStorageExt};
        use crate::witnessed::FoldedPlacement;
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(storage2.brand().handle()))
            .with_holder(&owned_cells)
    };
    let list_f = KObject::list_of_held(
        door,
        &[Held::Object(
            a_function(&storage2, second_run.scope, second_run.registries()).deep_clone(),
        )],
        &types,
    );
    let list_g = KObject::list_of_held(
        door,
        &[Held::Object(
            a_function(&storage2, second_run.scope, second_run.registries()).deep_clone(),
        )],
        &types,
    );
    assert_eq!(
        list_f.value_equal(&list_g, types.registries()),
        Err(ValueEqualityError::Function)
    );
}

#[test]
fn length_mismatch_short_circuits_before_banned_cell() {
    // The asymmetry the design accepts: a shape short-circuit that never reaches the banned
    // cell returns `Ok(false)` before any `Err`.
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;
    let program = program_storage();
    let storage = run_root_storage();
    let test_run = TestRun::silent(&program, &storage);
    let types = test_run.registry_handle();
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door = {
        use crate::machine::core::{FoldingBrand, FrameStorageExt};
        use crate::witnessed::FoldedPlacement;
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(storage.brand().handle()))
            .with_holder(&owned_cells)
    };
    let list_f = KObject::list_of_held(
        door,
        &[Held::Object(
            a_function(&storage, test_run.scope, types.registries()).deep_clone(),
        )],
        &types,
    );
    let empty = KObject::list(door, vec![], &types);
    assert_eq!(list_f.value_equal(&empty, types.registries()), Ok(false));
}

#[test]
fn module_operand_is_error() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;
    use crate::machine::model::SigSchema;
    use crate::machine::model::values::{Module, ModuleDraft};
    let program = program_storage();
    let storage = run_root_storage();
    let test_run = TestRun::silent(&program, &storage);
    let types = test_run.registry_handle();
    let draft = ModuleDraft::empty();
    let self_sig = types.signature(SigSchema::raw_self_sig(test_run.scope, &draft));
    let m = Module::alloc_at_child_scope("m", test_run.scope, draft, self_sig);
    let module = KObject::Module(m);
    assert_eq!(
        module.value_equal(&num(1.0), types.registries()),
        Err(ValueEqualityError::Module)
    );
    assert_eq!(
        num(1.0).value_equal(&module, types.registries()),
        Err(ValueEqualityError::Module)
    );
}
