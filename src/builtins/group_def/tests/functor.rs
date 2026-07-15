//! A functor — a module-returning `FN` — whose body is a `GROUP`. Instantiating it explicitly, at a
//! concrete type or at a witness module, yields a group module whose operators run over that
//! instantiation; opening it with `USING` puts `+ - …` in scope.
//!
//! Selection is always explicit: an instantiation binds a module, and only a `USING` window over
//! that module surfaces its operators. No group is chosen for a run by its operand type.

use super::list_numbers;
use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

/// A functor over a bare `:Type` parameter: the member bodies need no operation on `Elt`, so the
/// type alone parameterizes them. `+` returns its left operand and `-` its right, so a fold-left
/// `xs + ys - zs` = `(xs - zs)` = `zs` — both member bodies run.
const TYPE_PARAMETER_FUNCTOR: &str = "LET xs = [1]\n\
     LET ys = [2]\n\
     LET zs = [3]\n\
     LET make_ops = (FN (MAKEOPS Elt :Type) -> Module = (\
       GROUP result FOLD LEFT = (\
         (OP #(+) OVER :(LIST OF Elt) = (left))\
         (OP #(-) OVER :(LIST OF Elt) = (right)))))\n";

/// A functor over a witness module: the member bodies *do* need an operation on the element type, so
/// the instantiation passes a dictionary — a module satisfying `Additive` — and the `⊕` body calls
/// the operation the signature names. `⊖` subtracts, so a fold-left `2 ⊕ 3 ⊖ 1` runs both members.
const WITNESS_MODULE_FUNCTOR: &str = "SIG Additive = (\
       (TYPE Elt)\
       (VAL combine :(FN (x :Elt, y :Elt) -> Elt)))\n\
     MODULE sum_additive = (\
       (LET Elt = Number)\
       (LET combine = (FN (COMBINE x :Number y :Number) -> Number = (x + y))))\n\
     MODULE product_additive = (\
       (LET Elt = Number)\
       (LET combine = (FN (COMBINE x :Number y :Number) -> Number = (x * y))))\n\
     LET make_ops = (FN (MAKEOPS witness :Additive) -> Module = (\
       GROUP result FOLD LEFT = (\
         (OP #(⊕) OVER Number = (USING witness SCOPE (COMBINE left right)))\
         (OP #(⊖) OVER Number = (left - right)))))\n";

/// AC4, type-parameter form: `(MAKEOPS Number)` yields a group module whose `+` and `-` operate over
/// `:(LIST OF Number)`, and a mixed run inside a `USING` window over it reduces fold-left through
/// both member bodies.
#[test]
fn functor_instantiated_at_a_concrete_type_yields_operators_over_that_type() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        &format!(
            "{TYPE_PARAMETER_FUNCTOR}\
             LET number_ops = (MAKEOPS Number)\n\
             LET mixed = (USING number_ops SCOPE (xs + ys - zs))",
        ),
    );
    assert_eq!(
        list_numbers(run_one(scope, parse_one("mixed"))),
        vec![3.0],
        "the instantiated group's members reduce `xs + ys - zs` fold-left to `zs`",
    );
}

/// The `:Type` argument reaches the members' operand type: the same functor instantiated at `Str`
/// declares `+` and `-` over `:(LIST OF Str)`, so a run over number lists finds no member body — the
/// operand type is the one the instantiation supplied, not whatever the run happens to carry.
#[test]
fn functor_instantiated_at_another_type_does_not_admit_the_number_lists() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        &format!("{TYPE_PARAMETER_FUNCTOR}LET string_ops = (MAKEOPS Str)"),
    );
    let error = run_one_err(scope, parse_one("USING string_ops SCOPE (xs + ys - zs)"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
        "a `:(LIST OF Str)` member must not admit number lists, got {error}",
    );
}

/// Instantiation is explicit, and so is opening the result: a bound `number_ops` puts nothing in the
/// enclosing scope, so an unqualified run over the very operand type its members declare still
/// misses. Choosing a group for a run by the run's operand type is not a thing the language does.
#[test]
fn an_instantiated_group_is_opened_explicitly_never_selected_by_operand_type() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        &format!("{TYPE_PARAMETER_FUNCTOR}LET number_ops = (MAKEOPS Number)"),
    );
    let error = run_one_err(scope, parse_one("xs + ys - zs"));
    assert!(
        matches!(&error.kind, KErrorKind::DispatchFailed { .. }),
        "an instantiated-but-unopened group must not answer a run over its operand type, got {error}",
    );
    assert!(
        matches!(
            run_one(scope, parse_one("USING number_ops SCOPE (xs + ys - zs)")),
            KObject::List(..)
        ),
        "the same run inside a USING window over the instantiation reduces",
    );
}

/// AC4, witness-module form: two instantiations of one functor, at two dictionaries, give two groups
/// whose `⊕` runs the operation its own witness supplies — `(2 + 3) - 1` = 4 under `sum_additive`,
/// `(2 * 3) - 1` = 5 under `product_additive`. Both groups are live at once, each reached through its
/// own `USING` window.
#[test]
fn functor_instantiated_at_a_witness_module_yields_operators_over_that_witness() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        &format!(
            "{WITNESS_MODULE_FUNCTOR}\
             LET sum_ops = (MAKEOPS sum_additive)\n\
             LET product_ops = (MAKEOPS product_additive)\n\
             LET summed = (USING sum_ops SCOPE (2 ⊕ 3 ⊖ 1))\n\
             LET multiplied = (USING product_ops SCOPE (2 ⊕ 3 ⊖ 1))",
        ),
    );
    assert!(
        matches!(run_one(scope, parse_one("summed")), KObject::Number(n) if *n == 4.0),
        "`⊕` must combine through `sum_additive`, folding `2 ⊕ 3 ⊖ 1` to `(2 + 3) - 1`",
    );
    assert!(
        matches!(run_one(scope, parse_one("multiplied")), KObject::Number(n) if *n == 5.0),
        "the second instantiation's `⊕` combines through `product_additive`: `(2 * 3) - 1`",
    );
}
