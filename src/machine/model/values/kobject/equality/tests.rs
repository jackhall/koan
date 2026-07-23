use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::types::{KKind, KType, Record, RecursiveGroupWindow, RelativeSchema};
use crate::machine::model::values::{Held, KKey, KObject, ValueEqualityError};
use crate::machine::model::TypeRegistry;
use crate::source::Spanned;

fn num<'a>(n: f64) -> KObject<'a> {
    KObject::Number(n)
}

fn part(p: ExpressionPart<'static>) -> Spanned<ExpressionPart<'static>> {
    Spanned::bare(p)
}

fn newtype_singleton(name: &str, repr: KType, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(name.into(), RelativeSchema::NewType(repr), None, types)
}

/// Mint the zero-dep fold door a container test needs, over a fresh root region, as two `let`
/// bindings in the caller's own scope: `forge_for_test` is the sanctioned test-only placement
/// mint (no enclosing fold engine required). A statement macro (not a function returning the
/// pair) so `door`'s borrow of `storage` lives in the same frame it was minted in, never crossing
/// a return.
macro_rules! container_door {
    ($storage:ident, $door:ident) => {
        use crate::machine::core::{run_root_storage, FoldingBrand, FrameStorageExt};
        use crate::witnessed::FoldedPlacement;
        let $storage = run_root_storage();
        let $door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            $storage.brand().handle(),
        ));
    };
}

// --- scalars ----------------------------------------------------------------------

#[test]
fn number_ieee_semantics() {
    let types = TypeRegistry::new();
    assert_eq!(num(1.0).value_equal(&num(1.0), &types), Ok(true));
    assert_eq!(num(1.0).value_equal(&num(2.0), &types), Ok(false));
    // NaN is equal to nothing, including itself.
    assert_eq!(num(f64::NAN).value_equal(&num(f64::NAN), &types), Ok(false));
    // Signed zeros compare equal.
    assert_eq!(num(-0.0).value_equal(&num(0.0), &types), Ok(true));
}

#[test]
fn string_bool_null_scalars() {
    let types = TypeRegistry::new();
    let s = KObject::KString("a".into());
    assert_eq!(
        s.value_equal(&KObject::KString("a".into()), &types),
        Ok(true)
    );
    assert_eq!(
        s.value_equal(&KObject::KString("b".into()), &types),
        Ok(false)
    );
    assert_eq!(
        KObject::Bool(true).value_equal(&KObject::Bool(true), &types),
        Ok(true)
    );
    assert_eq!(
        KObject::Bool(true).value_equal(&KObject::Bool(false), &types),
        Ok(false)
    );
    assert_eq!(KObject::Null.value_equal(&KObject::Null, &types), Ok(true));
}

#[test]
fn cross_variant_scalars_are_unequal() {
    let types = TypeRegistry::new();
    assert_eq!(
        num(1.0).value_equal(&KObject::KString("a".into()), &types),
        Ok(false)
    );
    assert_eq!(KObject::Null.value_equal(&num(0.0), &types), Ok(false));
    assert_eq!(
        KObject::Bool(true).value_equal(&KObject::KString("true".into()), &types),
        Ok(false)
    );
}

// --- lists ------------------------------------------------------------------------

#[test]
fn list_element_and_length() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let a = KObject::list(door, vec![num(1.0), num(2.0)], &types);
    let b = KObject::list(door, vec![num(1.0), num(2.0)], &types);
    let c = KObject::list(door, vec![num(1.0), num(3.0)], &types);
    let short = KObject::list(door, vec![num(1.0)], &types);
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&c, &types), Ok(false));
    assert_eq!(a.value_equal(&short, &types), Ok(false));
}

#[test]
fn list_nan_self_compare_is_false() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    // No Rc-ptr fast path: a self-comparison of a NaN-holding list is element-wise false.
    let l = KObject::list(door, vec![num(f64::NAN)], &types);
    assert_eq!(l.value_equal(&l, &types), Ok(false));
}

#[test]
fn list_comparability_gate_is_intransitive() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    // `[]:Number` == `[]:Any` == `[]:Str`, but the outer two are unrelated → unequal.
    let empty_number =
        KObject::list(door, vec![], &types).stamp_type(types.list(KType::NUMBER), &types);
    let empty_any = KObject::list(door, vec![], &types).stamp_type(types.list(KType::ANY), &types);
    let empty_str = KObject::list(door, vec![], &types).stamp_type(types.list(KType::STR), &types);
    assert_eq!(empty_number.value_equal(&empty_any, &types), Ok(true));
    assert_eq!(empty_any.value_equal(&empty_str, &types), Ok(true));
    // Number and Str are unrelated → gate closes, no descent.
    assert_eq!(empty_number.value_equal(&empty_str, &types), Ok(false));
}

#[test]
fn list_of_types_compares_by_digest() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let a = KObject::list_of_held(door, vec![Held::Type(KType::NUMBER)], &types);
    let b = KObject::list_of_held(door, vec![Held::Type(KType::NUMBER)], &types);
    let c = KObject::list_of_held(door, vec![Held::Type(KType::STR)], &types);
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    // Different element type parameters (a `Type OF Number` vs `Type OF Str` list) close the gate.
    assert_eq!(a.value_equal(&c, &types), Ok(false));
}

// --- dicts ------------------------------------------------------------------------

fn dict(pairs: Vec<(KKey, KObject<'static>)>, types: &TypeRegistry) -> KObject<'static> {
    let mut map: HashMap<KKey, KObject<'static>> = HashMap::new();
    for (k, v) in pairs {
        map.insert(k, v);
    }
    KObject::dict(map, types)
}

#[test]
fn dict_key_and_value_equality() {
    let types = TypeRegistry::new();
    let a = dict(
        vec![
            (KKey::String("x".into()), num(1.0)),
            (KKey::String("y".into()), num(2.0)),
        ],
        &types,
    );
    let b = dict(
        vec![
            (KKey::String("y".into()), num(2.0)),
            (KKey::String("x".into()), num(1.0)),
        ],
        &types,
    );
    assert_eq!(a.value_equal(&b, &types), Ok(true));

    let missing_key = dict(
        vec![
            (KKey::String("x".into()), num(1.0)),
            (KKey::String("z".into()), num(2.0)),
        ],
        &types,
    );
    assert_eq!(a.value_equal(&missing_key, &types), Ok(false));

    let diff_value = dict(
        vec![
            (KKey::String("x".into()), num(1.0)),
            (KKey::String("y".into()), num(9.0)),
        ],
        &types,
    );
    assert_eq!(a.value_equal(&diff_value, &types), Ok(false));
}

#[test]
fn dict_length_mismatch_is_false() {
    let types = TypeRegistry::new();
    let a = dict(vec![(KKey::String("x".into()), num(1.0))], &types);
    let b = dict(
        vec![
            (KKey::String("x".into()), num(1.0)),
            (KKey::String("y".into()), num(2.0)),
        ],
        &types,
    );
    assert_eq!(a.value_equal(&b, &types), Ok(false));
}

// --- records ----------------------------------------------------------------------

fn record<'a>(
    door: crate::machine::core::FoldingBrand<'a>,
    pairs: Vec<(&str, KObject<'a>)>,
    types: &TypeRegistry,
) -> KObject<'a> {
    KObject::record(
        door,
        Record::from_pairs(pairs.into_iter().map(|(k, v)| (k.to_string(), v))),
        types,
    )
}

#[test]
fn record_field_order_blind_equality() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let a = record(door, vec![("x", num(1.0)), ("y", num(2.0))], &types);
    let b = record(door, vec![("y", num(2.0)), ("x", num(1.0))], &types);
    assert_eq!(a.value_equal(&b, &types), Ok(true));
}

#[test]
fn record_width_mismatch_comparable_but_unequal() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    // `{x:Number}` and `{x:Number, y:Number}` are related by record subtyping (gate open),
    // but the field sets differ → unequal.
    let narrow = record(door, vec![("x", num(1.0))], &types);
    let wide = record(door, vec![("x", num(1.0)), ("y", num(2.0))], &types);
    assert_eq!(narrow.value_equal(&wide, &types), Ok(false));
}

#[test]
fn record_field_value_differs() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let a = record(door, vec![("x", num(1.0))], &types);
    let b = record(door, vec![("x", num(2.0))], &types);
    assert_eq!(a.value_equal(&b, &types), Ok(false));
}

// --- tagged -----------------------------------------------------------------------

/// Two singleton newtype members declared together, so distinct handles exist for the
/// identity check. Returns the `None`-over-`Null` and `Some`-over-`Number` member handles.
fn two_member(types: &TypeRegistry) -> Vec<KType> {
    let window = RecursiveGroupWindow::new(
        vec![
            ("None".into(), KKind::NewType),
            ("Some".into(), KKind::NewType),
        ],
        None,
    );
    window.fill_member(0, RelativeSchema::NewType(KType::NULL), types);
    window
        .fill_member(1, RelativeSchema::NewType(KType::NUMBER), types)
        .expect("the last fill seals a fully declared window")
        .members
}

#[test]
fn tagged_same_nominal_compares_payload() {
    let types = TypeRegistry::new();
    let identity = newtype_singleton("Distance", KType::NUMBER, &types);
    let a = KObject::Tagged {
        tag: "Distance".into(),
        value: Rc::new(num(3.0)),
        identity,
    };
    let b = KObject::Tagged {
        tag: "Distance".into(),
        value: Rc::new(num(3.0)),
        identity,
    };
    let c = KObject::Tagged {
        tag: "Distance".into(),
        value: Rc::new(num(4.0)),
        identity,
    };
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&c, &types), Ok(false));
}

/// Identity-based equality reads an erased carrier (the bare member handle) and a stamped one
/// (a `ConstructorApply` over that member) as distinct types, so they compare unequal even with
/// equal payloads — the erased-vs-stamped distinction lives in the one identity handle.
#[test]
fn tagged_erased_and_stamped_are_distinct_identities() {
    let types = TypeRegistry::new();
    let ctor = RecursiveGroupWindow::seal_singleton(
        "Box".into(),
        RelativeSchema::TypeConstructor {
            schema: HashMap::new(),
            param_names: vec!["Type".into()],
        },
        None,
        &types,
    );
    let erased = KObject::Tagged {
        tag: "Box".into(),
        value: Rc::new(num(1.0)),
        identity: ctor,
    };
    let stamped = KObject::Tagged {
        tag: "Box".into(),
        value: Rc::new(num(1.0)),
        identity: types.constructor_apply(
            ctor,
            Record::from_pairs([("Type".to_string(), KType::NUMBER)]),
        ),
    };
    assert_eq!(erased.value_equal(&stamped, &types), Ok(false));
}

#[test]
fn tagged_distinct_index_is_unequal() {
    let types = TypeRegistry::new();
    let members = two_member(&types);
    let none = KObject::Tagged {
        tag: "None".into(),
        value: Rc::new(KObject::Null),
        identity: members[0],
    };
    let some = KObject::Tagged {
        tag: "Some".into(),
        value: Rc::new(num(1.0)),
        identity: members[1],
    };
    assert_eq!(none.value_equal(&some, &types), Ok(false));
}

// --- wrapped ----------------------------------------------------------------------

#[test]
fn wrapped_identity_and_payload() {
    let types = TypeRegistry::new();
    use crate::machine::model::values::WrappedPayload;
    let type_id = newtype_singleton("Distance", KType::NUMBER, &types);
    let a = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id,
    };
    let b = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id,
    };
    let diff_payload = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(4.0)),
        type_id,
    };
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&diff_payload, &types), Ok(false));
    // A wrapped value is never equal to its bare representation.
    assert_eq!(a.value_equal(&num(3.0), &types), Ok(false));
}

#[test]
fn wrapped_distinct_nominal_is_unequal() {
    let types = TypeRegistry::new();
    use crate::machine::model::values::WrappedPayload;
    let distance = newtype_singleton("Distance", KType::NUMBER, &types);
    let weight = newtype_singleton("Weight", KType::NUMBER, &types);
    let a = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id: distance,
    };
    let b = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id: weight,
    };
    assert_eq!(a.value_equal(&b, &types), Ok(false));
}

// --- expressions ------------------------------------------------------------------

#[test]
fn kexpression_structural_equality() {
    let types = TypeRegistry::new();
    let a = KObject::KExpression(KExpression::new(vec![
        part(ExpressionPart::Keyword("LET".into())),
        part(ExpressionPart::Identifier("x".into())),
    ]));
    let b = KObject::KExpression(KExpression::new(vec![
        part(ExpressionPart::Keyword("LET".into())),
        part(ExpressionPart::Identifier("x".into())),
    ]));
    let c = KObject::KExpression(KExpression::new(vec![
        part(ExpressionPart::Keyword("LET".into())),
        part(ExpressionPart::Identifier("y".into())),
    ]));
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&c, &types), Ok(false));
}

#[test]
fn kexpression_number_literal_is_ieee() {
    let types = TypeRegistry::new();
    let nan = KObject::KExpression(KExpression::new(vec![part(ExpressionPart::Literal(
        KLiteral::Number(f64::NAN),
    ))]));
    assert_eq!(nan.value_equal(&nan, &types), Ok(false));
    let one = KObject::KExpression(KExpression::new(vec![part(ExpressionPart::Literal(
        KLiteral::Number(1.0),
    ))]));
    let one2 = KObject::KExpression(KExpression::new(vec![part(ExpressionPart::Literal(
        KLiteral::Number(1.0),
    ))]));
    assert_eq!(one.value_equal(&one2, &types), Ok(true));
}

#[test]
fn kexpression_length_and_variant_mismatch() {
    let types = TypeRegistry::new();
    let a = KObject::KExpression(KExpression::new(vec![part(ExpressionPart::Keyword(
        "LET".into(),
    ))]));
    let longer = KObject::KExpression(KExpression::new(vec![
        part(ExpressionPart::Keyword("LET".into())),
        part(ExpressionPart::Identifier("x".into())),
    ]));
    // Different part variants at the same position.
    let variant = KObject::KExpression(KExpression::new(vec![part(ExpressionPart::Identifier(
        "LET".into(),
    ))]));
    assert_eq!(a.value_equal(&longer, &types), Ok(false));
    assert_eq!(a.value_equal(&variant, &types), Ok(false));
}

// --- banned operands --------------------------------------------------------------

/// A function value allocated in `storage`, closing over `scope` — the run root's own scope, so
/// the value is the one a real run would build.
fn a_function<'a>(
    storage: &'a Rc<crate::machine::core::FrameStorage>,
    scope: &'a crate::machine::Scope<'a>,
    types: &TypeRegistry,
) -> KObject<'a> {
    use crate::machine::core::{Body, FrameStorageExt};
    use crate::machine::model::types::{ExpressionSignature, ReturnType};
    use crate::machine::KFunction;
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::NUMBER),
        elements: Vec::new(),
    };
    let f = storage.brand().alloc_function(KFunction::new(
        sig,
        Body::UserDefined(KExpression::new(Vec::new())),
        scope,
        false,
        types,
    ));
    KObject::KFunction(f)
}

#[test]
fn function_operand_is_error_at_any_position() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;
    let storage = run_root_storage();
    let test_run = TestRun::silent(&storage);
    let types = test_run.types.clone();
    let f = a_function(&storage, test_run.scope, &types);
    assert_eq!(
        f.value_equal(&num(1.0), &types),
        Err(ValueEqualityError::Function)
    );
    assert_eq!(
        num(1.0).value_equal(&f, &types),
        Err(ValueEqualityError::Function)
    );
    // Nested: a function inside a list propagates the error.
    let storage2 = run_root_storage();
    let second_run = TestRun::silent(&storage2);
    let door = {
        use crate::machine::core::{FoldingBrand, FrameStorageExt};
        use crate::witnessed::FoldedPlacement;
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(storage2.brand().handle()))
    };
    let list_f = KObject::list_of_held(
        door,
        vec![Held::Object(
            a_function(&storage2, second_run.scope, &second_run.types).deep_clone(),
        )],
        &types,
    );
    let list_g = KObject::list_of_held(
        door,
        vec![Held::Object(
            a_function(&storage2, second_run.scope, &second_run.types).deep_clone(),
        )],
        &types,
    );
    assert_eq!(
        list_f.value_equal(&list_g, &types),
        Err(ValueEqualityError::Function)
    );
}

#[test]
fn length_mismatch_short_circuits_before_banned_cell() {
    // The asymmetry the design accepts: a shape short-circuit that never reaches the banned
    // cell returns `Ok(false)` before any `Err`.
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;
    let storage = run_root_storage();
    let test_run = TestRun::silent(&storage);
    let types = test_run.types.clone();
    let door = {
        use crate::machine::core::{FoldingBrand, FrameStorageExt};
        use crate::witnessed::FoldedPlacement;
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(storage.brand().handle()))
    };
    let list_f = KObject::list_of_held(
        door,
        vec![Held::Object(
            a_function(&storage, test_run.scope, &types).deep_clone(),
        )],
        &types,
    );
    let empty = KObject::list(door, vec![], &types);
    assert_eq!(list_f.value_equal(&empty, &types), Ok(false));
}

#[test]
fn module_operand_is_error() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;
    use crate::machine::model::values::Module;
    let storage = run_root_storage();
    let test_run = TestRun::silent(&storage);
    let types = test_run.types.clone();
    let m = Module::new("m".into(), test_run.scope);
    let module = KObject::Module(&m);
    assert_eq!(
        module.value_equal(&num(1.0), &types),
        Err(ValueEqualityError::Module)
    );
    assert_eq!(
        num(1.0).value_equal(&module, &types),
        Err(ValueEqualityError::Module)
    );
}
