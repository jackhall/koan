use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::ScopeId;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::types::{
    KKind, KType, NominalMember, NominalSchema, Record, RecursiveSet,
};
use crate::machine::model::values::{Held, KKey, KObject, ValueEqualityError};
use crate::machine::model::TypeRegistry;
use crate::source::Spanned;

fn num<'a>(n: f64) -> KObject<'a> {
    KObject::Number(n)
}

fn part(p: ExpressionPart<'static>) -> Spanned<ExpressionPart<'static>> {
    Spanned::bare(p)
}

fn newtype_singleton<'a>(name: &str, repr: KType<'a>) -> Rc<RecursiveSet<'a>> {
    RecursiveSet::singleton(
        name.into(),
        ScopeId::from_raw(0, 0xAA),
        NominalSchema::NewType(Box::new(repr)),
    )
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
    let a = KObject::list(vec![num(1.0), num(2.0)]);
    let b = KObject::list(vec![num(1.0), num(2.0)]);
    let c = KObject::list(vec![num(1.0), num(3.0)]);
    let short = KObject::list(vec![num(1.0)]);
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&c, &types), Ok(false));
    assert_eq!(a.value_equal(&short, &types), Ok(false));
}

#[test]
fn list_nan_self_compare_is_false() {
    let types = TypeRegistry::new();
    // No Rc-ptr fast path: a self-comparison of a NaN-holding list is element-wise false.
    let l = KObject::list(vec![num(f64::NAN)]);
    assert_eq!(l.value_equal(&l, &types), Ok(false));
}

#[test]
fn list_comparability_gate_is_intransitive() {
    let types = TypeRegistry::new();
    // `[]:Number` == `[]:Any` == `[]:Str`, but the outer two are unrelated → unequal.
    let empty_number = KObject::list_with_type(Rc::new(vec![]), KType::Number);
    let empty_any = KObject::list_with_type(Rc::new(vec![]), KType::Any);
    let empty_str = KObject::list_with_type(Rc::new(vec![]), KType::Str);
    assert_eq!(empty_number.value_equal(&empty_any, &types), Ok(true));
    assert_eq!(empty_any.value_equal(&empty_str, &types), Ok(true));
    // Number and Str are unrelated → gate closes, no descent.
    assert_eq!(empty_number.value_equal(&empty_str, &types), Ok(false));
}

#[test]
fn list_of_types_compares_by_digest() {
    let types = TypeRegistry::new();
    let a = KObject::list_of_held(vec![Held::Type(KType::Number)]);
    let b = KObject::list_of_held(vec![Held::Type(KType::Number)]);
    let c = KObject::list_of_held(vec![Held::Type(KType::Str)]);
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    // Different element type parameters (a `Type OF Number` vs `Type OF Str` list) close the gate.
    assert_eq!(a.value_equal(&c, &types), Ok(false));
}

// --- dicts ------------------------------------------------------------------------

fn dict(pairs: Vec<(KKey, KObject<'static>)>) -> KObject<'static> {
    let mut map: HashMap<KKey, KObject<'static>> = HashMap::new();
    for (k, v) in pairs {
        map.insert(k, v);
    }
    KObject::dict(map)
}

#[test]
fn dict_key_and_value_equality() {
    let types = TypeRegistry::new();
    let a = dict(vec![
        (KKey::String("x".into()), num(1.0)),
        (KKey::String("y".into()), num(2.0)),
    ]);
    let b = dict(vec![
        (KKey::String("y".into()), num(2.0)),
        (KKey::String("x".into()), num(1.0)),
    ]);
    assert_eq!(a.value_equal(&b, &types), Ok(true));

    let missing_key = dict(vec![
        (KKey::String("x".into()), num(1.0)),
        (KKey::String("z".into()), num(2.0)),
    ]);
    assert_eq!(a.value_equal(&missing_key, &types), Ok(false));

    let diff_value = dict(vec![
        (KKey::String("x".into()), num(1.0)),
        (KKey::String("y".into()), num(9.0)),
    ]);
    assert_eq!(a.value_equal(&diff_value, &types), Ok(false));
}

#[test]
fn dict_length_mismatch_is_false() {
    let types = TypeRegistry::new();
    let a = dict(vec![(KKey::String("x".into()), num(1.0))]);
    let b = dict(vec![
        (KKey::String("x".into()), num(1.0)),
        (KKey::String("y".into()), num(2.0)),
    ]);
    assert_eq!(a.value_equal(&b, &types), Ok(false));
}

// --- records ----------------------------------------------------------------------

fn record(pairs: Vec<(&str, KObject<'static>)>) -> KObject<'static> {
    KObject::record(Record::from_pairs(
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)),
    ))
}

#[test]
fn record_field_order_blind_equality() {
    let types = TypeRegistry::new();
    let a = record(vec![("x", num(1.0)), ("y", num(2.0))]);
    let b = record(vec![("y", num(2.0)), ("x", num(1.0))]);
    assert_eq!(a.value_equal(&b, &types), Ok(true));
}

#[test]
fn record_width_mismatch_comparable_but_unequal() {
    let types = TypeRegistry::new();
    // `{x:Number}` and `{x:Number, y:Number}` are related by record subtyping (gate open),
    // but the field sets differ → unequal.
    let narrow = record(vec![("x", num(1.0))]);
    let wide = record(vec![("x", num(1.0)), ("y", num(2.0))]);
    assert_eq!(narrow.value_equal(&wide, &types), Ok(false));
}

#[test]
fn record_field_value_differs() {
    let types = TypeRegistry::new();
    let a = record(vec![("x", num(1.0))]);
    let b = record(vec![("x", num(2.0))]);
    assert_eq!(a.value_equal(&b, &types), Ok(false));
}

// --- tagged -----------------------------------------------------------------------

fn two_member_set() -> Rc<RecursiveSet<'static>> {
    // A two-member set so distinct indices exist for the `same_nominal` identity check.
    let members = vec![
        NominalMember::pending("None".into(), ScopeId::from_raw(0, 0xBB), KKind::NewType),
        NominalMember::pending("Some".into(), ScopeId::from_raw(0, 0xBB), KKind::NewType),
    ];
    let set = RecursiveSet::new(members);
    set.fill_member(0, NominalSchema::NewType(Box::new(KType::Null)));
    set.fill_member(1, NominalSchema::NewType(Box::new(KType::Number)));
    Rc::new(set)
}

#[test]
fn tagged_same_nominal_compares_payload() {
    let types = TypeRegistry::new();
    let set = newtype_singleton("Distance", KType::Number);
    let a = KObject::Tagged {
        tag: "Distance".into(),
        value: Rc::new(num(3.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![]),
    };
    let b = KObject::Tagged {
        tag: "Distance".into(),
        value: Rc::new(num(3.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![]),
    };
    let c = KObject::Tagged {
        tag: "Distance".into(),
        value: Rc::new(num(4.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![]),
    };
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&c, &types), Ok(false));
}

#[test]
fn tagged_erased_vs_stamped_is_comparable() {
    let types = TypeRegistry::new();
    // Empty type_args on one side = erased = comparable; the payloads decide.
    let set = newtype_singleton("Box", KType::Number);
    let erased = KObject::Tagged {
        tag: "Box".into(),
        value: Rc::new(num(1.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![]),
    };
    let stamped = KObject::Tagged {
        tag: "Box".into(),
        value: Rc::new(num(1.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![KType::Number]),
    };
    assert_eq!(erased.value_equal(&stamped, &types), Ok(true));
}

#[test]
fn tagged_distinct_index_is_unequal() {
    let types = TypeRegistry::new();
    let set = two_member_set();
    let none = KObject::Tagged {
        tag: "None".into(),
        value: Rc::new(KObject::Null),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![]),
    };
    let some = KObject::Tagged {
        tag: "Some".into(),
        value: Rc::new(num(1.0)),
        set: Rc::clone(&set),
        index: 1,
        type_args: Rc::new(vec![]),
    };
    assert_eq!(none.value_equal(&some, &types), Ok(false));
}

// --- wrapped ----------------------------------------------------------------------

#[test]
fn wrapped_identity_and_payload() {
    let types = TypeRegistry::new();
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::WrappedPayload;
    let storage = run_root_storage();
    let region = storage.brand();
    let set = newtype_singleton("Distance", KType::Number);
    let type_id_a = region.alloc_ktype(KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    });
    let type_id_b = region.alloc_ktype(KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    });
    let a = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id: type_id_a,
    };
    let b = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id: type_id_b,
    };
    let diff_payload = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(4.0)),
        type_id: type_id_a,
    };
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&diff_payload, &types), Ok(false));
    // A wrapped value is never equal to its bare representation.
    assert_eq!(a.value_equal(&num(3.0), &types), Ok(false));
}

#[test]
fn wrapped_distinct_nominal_is_unequal() {
    let types = TypeRegistry::new();
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::WrappedPayload;
    let storage = run_root_storage();
    let region = storage.brand();
    let distance = newtype_singleton("Distance", KType::Number);
    let weight = newtype_singleton("Weight", KType::Number);
    let a = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id: region.alloc_ktype(KType::SetRef {
            set: distance,
            index: 0,
        }),
    };
    let b = KObject::Wrapped {
        inner: WrappedPayload::hold(&num(3.0)),
        type_id: region.alloc_ktype(KType::SetRef {
            set: weight,
            index: 0,
        }),
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

fn a_function(storage: &Rc<crate::machine::core::FrameStorage>) -> KObject<'_> {
    use crate::builtins::default_scope;
    use crate::machine::core::{Body, FrameStorageExt};
    use crate::machine::model::types::{ExpressionSignature, ReturnType};
    use crate::machine::KFunction;
    let scope = default_scope(storage, Box::new(std::io::sink()));
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: Vec::new(),
    };
    let f = storage.brand().alloc_function(KFunction::new(
        sig,
        Body::UserDefined(KExpression::new(Vec::new())),
        scope,
        None,
        None,
    ));
    KObject::KFunction(f)
}

#[test]
fn function_operand_is_error_at_any_position() {
    let types = TypeRegistry::new();
    use crate::machine::core::run_root_storage;
    let storage = run_root_storage();
    let f = a_function(&storage);
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
    let list_f = KObject::list_of_held(vec![Held::Object(a_function(&storage2).deep_clone())]);
    let list_g = KObject::list_of_held(vec![Held::Object(a_function(&storage2).deep_clone())]);
    assert_eq!(
        list_f.value_equal(&list_g, &types),
        Err(ValueEqualityError::Function)
    );
}

#[test]
fn length_mismatch_short_circuits_before_banned_cell() {
    let types = TypeRegistry::new();
    // The asymmetry the design accepts: a shape short-circuit that never reaches the banned
    // cell returns `Ok(false)` before any `Err`.
    use crate::machine::core::run_root_storage;
    let storage = run_root_storage();
    let list_f = KObject::list_of_held(vec![Held::Object(a_function(&storage).deep_clone())]);
    let empty = KObject::list(vec![]);
    assert_eq!(list_f.value_equal(&empty, &types), Ok(false));
}

#[test]
fn module_operand_is_error() {
    let types = TypeRegistry::new();
    use crate::builtins::default_scope;
    use crate::machine::core::run_root_storage;
    use crate::machine::model::values::Module;
    let storage = run_root_storage();
    let scope = default_scope(&storage, Box::new(std::io::sink()));
    let m = Module::new("m".into(), scope);
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
