//! End-to-end tests for the type-language-via-dispatch sigil surface.
//!
//! Covers the keyworded type-constructor overloads (`LIST OF`,
//! `MAP _ -> _`, `FN`) registered by
//! [`koan::builtins::type_constructors`], plus the legacy `:(List Number)`
//! positional fallback served by the dispatcher's `TypeCall` arm.
//!
//! These exercise the *sigil boundary*: a `:(...)` expression evaluates its
//! inner expression through the standard dispatch classifier and the result is
//! a type-side carrier (`KTypeValue` for structural types, `Module` /
//! `Signature` / `SetMember` for nominal identities) that downstream slots
//! type-check naturally.
//!
//! Companion design: [design/typing/type-language-via-dispatch.md].

use std::rc::Rc;

use koan::builtins::test_support::{TestRun, lookup_binding, lookup_type};
use koan::machine::model::{
    KKind, KObject, KType, NodeSchema, Symbol, TypeNode, TypeRegistry, ValueSymbol,
};
use koan::machine::{FrameStorage, ProgramStorage, Scope, program_storage, run_root_storage};

/// Run `src` to completion and hand back the whole run — the seeded scope tests assert
/// bindings on, plus the run frame's registry that type names render against.
fn run<'a>(program: &'a ProgramStorage, region: &'a Rc<FrameStorage>, src: &str) -> TestRun<'a> {
    let mut test_run = TestRun::silent(program, region);
    let scope = test_run.scope;
    test_run.dispatch_source_in(scope, src);
    test_run
        .runtime
        .execute()
        .expect("scheduler should run to completion");
    test_run
}

fn run_expect_err(program: &ProgramStorage, region: &Rc<FrameStorage>, src: &str) -> String {
    let mut test_run = TestRun::silent(program, region);
    let scope = test_run.scope;
    let ids = test_run.dispatch_source_in(scope, src);
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let last = *ids.last().expect("at least one expression");
    match test_run.runtime.edge_result_error(last) {
        Ok(()) => panic!("expected scheduler error, got success"),
        Err(e) => e.to_string(),
    }
}

/// Read the SIG named `sig_name`'s value slot for `name` as its declared `KType`. Reads the
/// run-root scope's type side (`resolve_type`) to grab the Signature carrier, then reads the
/// Signature's stored schema (`value_slots`) — where VAL value slots record their declared type
/// under their value-class name.
fn lookup_sig_value_kt(
    scope: &Scope<'_>,
    types: &TypeRegistry,
    sig_name: &str,
    name: &str,
) -> KType {
    let handle = lookup_type(scope, sig_name)
        .unwrap_or_else(|| panic!("`{sig_name}` should bind a Signature KType, got nothing"));
    let schema = match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("`{sig_name}` should bind a Signature KType, got {handle:?}"),
    };
    ValueSymbol::classify(name)
        .and_then(|slot| schema.value_slots.get(&slot).copied())
        .unwrap_or_else(|| panic!("`{name}` should be bound in the SIG's stored schema"))
}

// --- LIST OF ---

/// `:(LIST OF Number)` lowers to `KType::List(Number)` and binds via VAL.
#[test]
fn sigil_list_of_lowers_to_list_carrier() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "SIG Holder = ((VAL items :(LIST OF Number)))",
    );
    let items_kt = lookup_sig_value_kt(test_run.scope, test_run.types(), "Holder", "items");
    match test_run.types().node(items_kt) {
        TypeNode::List { element } => assert_eq!(element, KType::NUMBER),
        _ => panic!("items must be KType::List(Number), got {items_kt:?}"),
    }
}

/// `:(LIST Number)` (no `OF`) is a dispatch error — the all-uppercase Keyword
/// `LIST` has no overload registered without the connector keyword `OF`.
#[test]
fn sigil_list_of_missing_of_keyword_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let err = run_expect_err(&program, &region, "LET Ty = :(LIST Number)");
    assert!(
        err.contains("dispatch failed") || err.contains("no matching function"),
        "expected DispatchFailed surface, got: {err}",
    );
}

// --- MAP _ -> _ ---

/// `:(MAP Str -> Number)` lowers to `KType::Dict(Str, Number)`. Surface keyword
/// changed; underlying carrier identity is unchanged from the legacy `Dict`.
#[test]
fn sigil_map_lowers_to_dict_carrier() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "SIG Holder = ((VAL table :(MAP Str -> Number)))",
    );
    let table_kt = lookup_sig_value_kt(test_run.scope, test_run.types(), "Holder", "table");
    match test_run.types().node(table_kt) {
        TypeNode::Dict { key, value } => {
            assert_eq!(key, KType::STR);
            assert_eq!(value, KType::NUMBER);
        }
        _ => panic!("table must be KType::Dict(Str, Number), got {table_kt:?}"),
    }
}

// --- FN ---

/// `:(FN :{x :Number, y :Str} -> Bool)` lowers to a `KType::KFunction { params, ret }`
/// whose `params` record keys each parameter type by its declared name.
#[test]
fn sigil_fn_lowers_to_kfunction_named() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "SIG Holder = ((VAL compare :(FN :{x :Number, y :Str} -> Bool)))",
    );
    let cmp = lookup_sig_value_kt(test_run.scope, test_run.types(), "Holder", "compare");
    match test_run.types().node(cmp) {
        TypeNode::KFunction { params, ret } => {
            assert_eq!(params.len(), 2);
            assert_eq!(params.get(Symbol::of("x")), Some(&KType::NUMBER));
            assert_eq!(params.get(Symbol::of("y")), Some(&KType::STR));
            assert_eq!(ret, KType::BOOL);
        }
        _ => panic!("compare must be KType::KFunction, got {cmp:?}"),
    }
}

/// Nullary FN: `:(FN :{} -> Number)` lowers to a zero-arg function type.
#[test]
fn sigil_fn_nullary_lowers_to_zero_arg_kfunction() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "SIG Holder = ((VAL gen :(FN :{} -> Number)))",
    );
    let r#gen = lookup_sig_value_kt(test_run.scope, test_run.types(), "Holder", "gen");
    match test_run.types().node(r#gen) {
        TypeNode::KFunction { params, ret } => {
            assert!(params.is_empty());
            assert_eq!(ret, KType::NUMBER);
        }
        _ => panic!("gen must be KType::KFunction, got {gen:?}"),
    }
}

/// A functor — a module-returning function — types as an ordinary `:(FN …)`: the capitalized
/// `Type`-token parameter name `Ty` keys the params record, and `-> Module` lowers to the
/// empty signature.
#[test]
fn sigil_fn_type_param_and_module_return_lowers_to_kfunction() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "SIG Holder = ((VAL mk :(FN :{Ty :Signature} -> Module)))",
    );
    let mk = lookup_sig_value_kt(test_run.scope, test_run.types(), "Holder", "mk");
    match test_run.types().node(mk) {
        TypeNode::KFunction { params, ret } => {
            assert_eq!(params.len(), 1);
            assert_eq!(
                params.get(Symbol::of("Ty")),
                Some(&KType::of_kind(KKind::Signature))
            );
            assert_eq!(ret, KType::EMPTY_SIGNATURE);
        }
        _ => panic!("mk must be KType::KFunction, got {mk:?}"),
    }
}

/// `:(FN …)` is koan's only function-type surface: `FUNCTOR` registers no overload, so a
/// `:(FUNCTOR …)` sigil is an ordinary dispatch no-match.
#[test]
fn sigil_functor_is_unbound() {
    let program = program_storage();
    let region = run_root_storage();
    let err = run_expect_err(
        &program,
        &region,
        "SIG Holder = ((VAL mk :(FUNCTOR (Ty :Signature) -> Module)))",
    );
    assert!(
        err.contains("FUNCTOR"),
        "the unbound `:(FUNCTOR …)` sigil should surface a dispatch no-match naming FUNCTOR, \
         got: {err}",
    );
}

// `sigil_legacy_list_number_falls_through_typecall` deleted: the legacy
// positional shape no longer routes through the dispatcher (TypeCall arm
// removed). Field schemas inside STRUCT/UNION still elaborate the legacy form
// inline via `try_synth_legacy`, but a standalone `:(List Number)` no longer
// resolves through the standalone dispatch path.

// --- Keyworded sigiled type expressions inside STRUCT/UNION field schemas ---

/// `NEWTYPE Foo = :{xs :(LIST OF Number)}` — the keyworded `LIST OF` sigil sub-Dispatches
/// through the dispatcher, producing a `KTypeValue(List<Number>)` carrier that the
/// field-walker splices back as the field's resolved KType inside the record repr.
#[test]
fn newtype_record_field_accepts_keyworded_list_of_sigil() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(&program, &region, "NEWTYPE Foo = :{xs :(LIST OF Number)}");
    // NEWTYPE is type-only — its record repr rides the sealed `SetMember` in `types`.
    let foo = lookup_type(test_run.scope, "Foo").expect("Foo must resolve to a type");
    let fields = match test_run.types().node(foo) {
        TypeNode::SetMember {
            schema: NodeSchema::NewType(repr),
            ..
        } => match test_run.types().node(repr) {
            TypeNode::Record { fields } => fields,
            _ => panic!("Foo must project a record-repr NewType schema"),
        },
        _ => panic!("Foo must be a NewType SetMember in types, got {foo:?}"),
    };
    assert_eq!(fields.len(), 1);
    let (xs_name, xs_type) = fields.iter().next().expect("one field");
    assert_eq!(xs_name.symbol(), Symbol::of("xs"));
    match test_run.types().node(*xs_type) {
        TypeNode::List { element } => assert_eq!(element, KType::NUMBER),
        _ => panic!("xs must be KType::List(Number), got {xs_type:?}"),
    }
}

/// `UNION Maybe = (Some :(MAP Str -> Number), None :Null)` — keyworded `MAP` sigil
/// inside a UNION field. Same sub-Dispatch path, different field-walker invocation.
#[test]
fn union_field_accepts_keyworded_map_sigil() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "UNION Maybe = (Some :(MAP Str -> Number), None :Null)",
    );
    // UNION is type-only — it binds an anonymous union of per-variant newtypes; the `Some`
    // variant's newtype repr is the keyworded `MAP` sigil that sub-Dispatched.
    let maybe = lookup_type(test_run.scope, "Maybe").expect("Maybe must resolve to a type");
    let some_repr = match test_run.types().node(maybe) {
        TypeNode::Union { members } => members
            .iter()
            .find_map(|m| match test_run.types().node(*m) {
                TypeNode::SetMember {
                    name,
                    schema: NodeSchema::NewType(repr),
                    ..
                } if name.symbol() == koan::machine::model::Symbol::of("Some") => Some(repr),
                _ => None,
            })
            .expect("Some variant must project a NewType repr"),
        _ => panic!("Maybe must be a Union in types, got {maybe:?}"),
    };
    match test_run.types().node(some_repr) {
        TypeNode::Dict { key, value } => {
            assert_eq!(key, KType::STR);
            assert_eq!(value, KType::NUMBER);
        }
        _ => panic!("Some repr must be KType::Dict(Str, Number), got {some_repr:?}"),
    }
}

// --- User-functor application ---

/// A functor — a module-returning FN — binds a `KFunction` carrier under `MAKESET`, and the
/// keyworded call `(MAKESET int_ord)` produces a Module value. Ordinary dispatch, no type-side
/// surface: the test pins that a functor application rides the same lane as any other call.
#[test]
fn user_functor_application_through_dispatch() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = run(
        &program,
        &region,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord_base = ((LET compare = 7))\n\
         LET int_ord = (int_ord_base :! Ordered)\n\
         FN (MAKESET er :Ordered) -> Module = \
            (MODULE generated = ((LET tag = 0)))\n\
         LET my_set = (MAKESET int_ord)",
    );
    // `my_set` is a module bound under a Type-classed name — a module is a value, so it binds
    // on the value channel.
    assert!(
        matches!(
            lookup_binding(test_run.scope, "my_set"),
            Some(KObject::Module(_))
        ),
        "my_set must bind the produced module value",
    );
}
