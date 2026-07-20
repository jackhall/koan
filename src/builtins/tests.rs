//! The kind check every value type position shares: a value's type must be a proper type
//! (kind `*`), so a bare type constructor — kind `* -> *`, standing with none of its parameters
//! supplied — is rejected wherever a value's type is declared.
//!
//! The rule is one predicate ([`unsaturated_constructor_message`]) applied at each surface that
//! declares one, so the surfaces are covered together here rather than split across the builtins
//! that host them. A *type* position takes a bare constructor legitimately and is covered by the
//! acceptance half.
//!
//! [`unsaturated_constructor_message`]: crate::machine::model::unsaturated_constructor_message

use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

/// Two families the whole file declares against: `Wrapper` over one parameter and `Pair` over
/// two, both concrete (`NEWTYPE`-declared) constructors.
const FAMILIES: &str = "NEWTYPE (Elem AS Wrapper)\nNEWTYPE (Key Val AS Pair)";

/// The message the gate renders, built from the same parts the diagnostic does: the constructor's
/// name and parameter list, then `position` — the noun phrase naming the type slot, which must read
/// as the subject of "must be a proper type".
fn expected_kind_error(constructor: &str, params: &[&str], position: &str) -> String {
    let plural = if params.len() == 1 { "" } else { "s" };
    let listed = params
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let applied = params
        .iter()
        .map(|p| format!("{p} = <Type>"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "`{constructor}` is a type constructor taking {arity} type parameter{plural} ({listed}), \
         but {position} must be a proper type — apply it, as `:({constructor} {{{applied}}})`",
        arity = params.len(),
    )
}

/// Assert `source` (run after `setup`) fails with exactly the kind error for `constructor` at
/// `position` — the whole message, so a label that stops naming the construct the user wrote, or
/// stops reading grammatically in the template, fails here.
#[track_caller]
fn assert_kind_error_after(
    setup: &str,
    source: &str,
    constructor: &str,
    params: &[&str],
    position: &str,
) {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, setup);
    let error = run_one_err(scope, parse_one(source));
    let KErrorKind::ShapeError(message) = &error.kind else {
        panic!("expected a ShapeError for `{source}`, got {error}");
    };
    assert_eq!(
        message,
        &expected_kind_error(constructor, params, position),
        "`{source}` rendered the wrong kind diagnostic",
    );
}

/// [`assert_kind_error_after`] against [`FAMILIES`], the setup most cases share.
#[track_caller]
fn assert_kind_error(source: &str, constructor: &str, params: &[&str], position: &str) {
    assert_kind_error_after(FAMILIES, source, constructor, params, position);
}

/// Assert `source` (run after `setup`) raises no error — the well-kinded half.
#[track_caller]
fn assert_accepted(setup: &str, source: &str) {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, setup);
    let mut runtime = crate::machine::KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one(source), scope);
    runtime.execute().expect("scheduler should succeed");
    if let Err(error) = runtime.result_error(id) {
        panic!("`{source}` must be accepted, got {error}");
    }
}

// ---------- rejection: every value type position ----------

#[test]
fn sig_value_slot_rejects_bare_constructor() {
    assert_kind_error(
        "SIG Boxy = ((VAL boxed :Wrapper))",
        "Wrapper",
        &["Elem"],
        "the type of SIG value slot `boxed`",
    );
    assert_kind_error(
        "SIG Boxy = ((VAL boxed :Pair))",
        "Pair",
        &["Key", "Val"],
        "the type of SIG value slot `boxed`",
    );
}

/// The SIG-body value slot rejects a bare *abstract* constructor too: a higher-kinded `TYPE
/// (Item AS Wrap)` member is kind `* -> *` in the same way a `NEWTYPE`-declared family is, and
/// the slot it stands in is labelled identically.
#[test]
fn sig_value_slot_rejects_bare_abstract_constructor() {
    assert_kind_error_after(
        "",
        "SIG Cont = ((TYPE (Item AS Wrap)) (VAL one :Wrap))",
        "Wrap",
        &["Item"],
        "the type of SIG value slot `one`",
    );
}

#[test]
fn fn_parameter_rejects_bare_constructor() {
    assert_kind_error(
        "FN (ECHO x :Wrapper) -> Number = (1.0)",
        "Wrapper",
        &["Elem"],
        "the type of FN parameter `x`",
    );
    assert_kind_error(
        "FN (ECHO x :Pair) -> Number = (1.0)",
        "Pair",
        &["Key", "Val"],
        "the type of FN parameter `x`",
    );
}

#[test]
fn fn_return_type_rejects_bare_constructor() {
    assert_kind_error(
        "FN (ECHO x :Number) -> Wrapper = (1.0)",
        "Wrapper",
        &["Elem"],
        "the FN return type",
    );
}

/// The anonymous `FN :{…}` form declares its parameters as a record type — the `:{…}` elaborates
/// as an ordinary record type before `FN` sees it — so the record-type field label is what names
/// the slot, and the same rule gates it.
#[test]
fn anonymous_fn_record_parameter_rejects_bare_constructor() {
    assert_kind_error(
        "LET f = (FN :{x :Wrapper} -> Number = (1.0))",
        "Wrapper",
        &["Elem"],
        "the type of record-type field `x`",
    );
}

#[test]
fn op_operand_and_result_reject_bare_constructor() {
    assert_kind_error(
        "OP #(+) OVER Wrapper = (1.0)",
        "Wrapper",
        &["Elem"],
        "the OP operand type",
    );
    assert_kind_error(
        "UNARY OP #(-) OVER Number -> Pair = (1.0)",
        "Pair",
        &["Key", "Val"],
        "the OP result type",
    );
}

#[test]
fn record_type_field_rejects_bare_constructor() {
    assert_kind_error(
        "LET Rec = :{x :Wrapper}",
        "Wrapper",
        &["Elem"],
        "the type of record-type field `x`",
    );
}

#[test]
fn union_variant_payload_rejects_bare_constructor() {
    assert_kind_error(
        "UNION Shape = (Circle :Wrapper Square :Number)",
        "Wrapper",
        &["Elem"],
        "the type of UNION variant `Circle`",
    );
}

#[test]
fn newtype_representation_rejects_bare_constructor() {
    assert_kind_error(
        "NEWTYPE Boxed = Wrapper",
        "Wrapper",
        &["Elem"],
        "the representation type of NEWTYPE `Boxed`",
    );
    assert_kind_error(
        "NEWTYPE Boxed = :{v :Pair}",
        "Pair",
        &["Key", "Val"],
        "the type of NEWTYPE repr field `v`",
    );
}

/// The type language's own argument positions demand kind `*` for the same reason: a list's
/// element, a dict's key and value, and a function type's parameters and return each name the
/// type of a value.
#[test]
fn type_language_argument_positions_reject_bare_constructor() {
    assert_kind_error(
        "LET Xs = :(LIST OF Wrapper)",
        "Wrapper",
        &["Elem"],
        "the element type of `LIST OF`",
    );
    assert_kind_error(
        "LET Mk = :(MAP Wrapper -> Number)",
        "Wrapper",
        &["Elem"],
        "the key type of `MAP`",
    );
    assert_kind_error(
        "LET Mv = :(MAP Str -> Pair)",
        "Pair",
        &["Key", "Val"],
        "the value type of `MAP`",
    );
    assert_kind_error(
        "LET Ft = :(FN (x :Number) -> Wrapper)",
        "Wrapper",
        &["Elem"],
        "the return type of an `:(FN …)` type",
    );
    assert_kind_error(
        "LET Ft = :(FN (x :Wrapper) -> Number)",
        "Wrapper",
        &["Elem"],
        "the type of FN parameter `x`",
    );
}

// ---------- acceptance: saturated, first-order, and genuine type positions ----------

/// A saturated application is a proper type in every gated position, by either spelling —
/// `:(Wrapper {Elem = Number})` and its arity-1 `AS` sugar.
#[test]
fn saturated_application_is_accepted_in_value_positions() {
    for source in [
        "SIG Boxy = ((VAL boxed :(Wrapper {Elem = Number})))",
        "FN (ECHO x :(Number AS Wrapper)) -> Number = (1.0)",
        "FN (ECHO x :Number) -> :(Wrapper {Elem = Str}) = (1.0)",
        "LET Rec = :{x :(Number AS Wrapper)}",
        "LET Xs = :(LIST OF (Number AS Wrapper))",
        "NEWTYPE Boxed = :{v :(Pair {Key = Str, Val = Number})}",
    ] {
        assert_accepted(FAMILIES, source);
    }
}

/// A SIG's *first-order* abstract member carries no parameters, so it is a proper type and fills
/// a value slot — the shape `SIG Ordered = ((TYPE Elem) (VAL zero :Elem))` depends on.
#[test]
fn first_order_abstract_member_fills_a_value_slot() {
    assert_accepted("", "SIG Ordered = ((TYPE Elem) (VAL zero :Elem))");
    assert_accepted(
        "",
        "SIG Ordered = ((TYPE Elem) (VAL compare :(FN (a :Elem b :Elem) -> Bool)))",
    );
}

/// A SIG's higher-kinded abstract member is legal in the *head* of an application inside a value
/// slot — only the bare, unapplied spelling is a kind error.
#[test]
fn higher_kinded_abstract_member_is_applied_in_a_value_slot() {
    assert_accepted(
        "",
        "SIG Cont = ((TYPE (Item AS Wrap)) (VAL one :(Number AS Wrap)))",
    );
}

/// A bare constructor in a genuine *type* position stays legal: a `LET` binding it under a
/// Type-classified name is how a module supplies a type-constructor member.
#[test]
fn bare_constructor_is_accepted_in_a_type_position() {
    assert_accepted(FAMILIES, "LET Wrap = Wrapper");
    assert_accepted(FAMILIES, "LET Couple = Pair");
}

/// Ordinary first-order types are untouched across the gated surfaces.
#[test]
fn first_order_types_are_accepted_across_gated_surfaces() {
    for source in [
        "SIG Boxy = ((VAL boxed :Number))",
        "FN (ECHO xs :(LIST OF Number)) -> Number = (1.0)",
        "FN (ECHO f :(FN (x :Number) -> Number)) -> Any = (1.0)",
        "FN (ECHO r :{a :Number}) -> Str = (\"ok\")",
        "OP #(+) OVER Number = (1.0)",
        "UNION Shape = (Circle :Number Square :Str)",
        "NEWTYPE Boxed = :{v :Str}",
        "LET Mp = :(MAP Str -> Number)",
    ] {
        assert_accepted(FAMILIES, source);
    }
}
