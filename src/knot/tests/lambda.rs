//! Lambdas: a callable no binder names born alone through the door, and a `FN` a data member holds
//! that captures a fellow member born inside that member's knot.

use crate::memory::Writer;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{BodyShape, CaptureSlot, Site};
use crate::symbols::TypeSymbol;
use crate::type_lattice::KType;
use crate::values::{Circular, Knotted as _, Link, Value};

use super::super::{KActivation, Knotted, Untieable, lambda};
use super::birth::tie_with;
use super::{Fixture, bound, callable, circular, follow, pin, with_fixture};

/// The `FN` node statement `statement` of `shape` is.
fn statement_lambda<'graph>(
    shape: &BodyShape<'graph>,
    statement: usize,
) -> &'graph KExpression<'graph> {
    shape.body()[statement].statement_spine()
}

/// Item `item` of the list literal statement `statement` binds.
fn item_lambda<'graph>(
    shape: &BodyShape<'graph>,
    statement: usize,
    item: usize,
) -> &'graph ExpressionPart<'graph> {
    let ExpressionPart::ListLiteral(items) =
        shape.body()[statement].statement_spine().parts[3].value
    else {
        panic!("the statement binds a list literal");
    };
    &items[item]
}

/// The body shape of the `FN` node `part` is.
fn body_of<'graph>(
    shape: &BodyShape<'graph>,
    part: &ExpressionPart<'graph>,
) -> &'graph BodyShape<'graph> {
    let ExpressionPart::Expression(node) = part else {
        panic!("a `FN` item is a parenthesized node");
    };
    let site = Site::of_body(node.reference()).expect("a `FN` has a body");
    shape.nested(site).expect("the body has a shape")
}

/// Statement `statement` of `activation`'s shape, a `FN`, born through the door.
fn born<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    writer: Writer<'cell>,
    activation: &KActivation<'graph, 'cell>,
    statement: usize,
) -> Knotted<'graph, 'cell> {
    let node = statement_lambda(activation.shape(), statement);
    let site = Site::of_body(node).expect("a `FN` has a body");
    lambda(writer, activation, site, fixture.types, fixture.scratch()).expect("the lambda is born")
}

#[test]
fn a_lambda_is_born_as_a_one_node_knot() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = \"kept\"\n(FN :{} -> Str = (k))");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let member = born(fixture, writer, activation, 1);
            assert_eq!(member.member().knot().len(), 1);
            let function = member.function().expect("a function");
            let shape = activation.shape();
            let node = statement_lambda(shape, 1);
            assert!(std::ptr::eq(
                function.shape(),
                shape.nested(Site::of_body(node).unwrap()).unwrap()
            ));
            assert_eq!(
                member.ktype(),
                fixture
                    .types
                    .function_type(fixture.scratch(), &[], &[], KType::STR)
                    .handle
            );
            let Link::Value(Value::Str(captured)) = function.closure().get(CaptureSlot(0)) else {
                panic!("`k` is captured as its value word");
            };
            let Value::Str(slot) = bound(fixture, activation, "k") else {
                panic!("`k` is a string");
            };
            assert!(std::ptr::eq(captured, slot), "the word, not a copy");
        });
    });
}

#[test]
fn a_lambda_reads_a_later_binding_at_birth() {
    with_fixture(|fixture| {
        let lines = fixture.parse("(FN :{} -> Number = (later))\nLET later = 5");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let member = born(fixture, writer, activation, 0);
            let closure = member.function().expect("a function").closure();
            assert!(matches!(
                closure.get(CaptureSlot(0)),
                Link::Value(Value::Number(5.0))
            ));
        });
    });
}

#[test]
fn a_lambda_weighs_what_the_tie_gives_the_same_function() {
    with_fixture(|fixture| {
        let lines =
            fixture.parse("LET k = \"kept\"\nLET f = (FN :{} -> Str = (k))\n(FN :{} -> Str = (k))");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let member = born(fixture, writer, activation, 2);
            let f = callable(fixture, activation, "f");
            assert_eq!(member.ktype(), f.ktype());
            assert_eq!(member.weight(), f.weight());
        });
    });
}

#[test]
fn a_quantified_lambda_carries_its_quantifier_map() {
    with_fixture(|fixture| {
        let lines = fixture.parse("(FN FOR ALL (Elt) :{x :Elt} -> Elt = (x))");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let member = born(fixture, writer, activation, 0);
            let function = member.function().expect("a function");
            assert_eq!(function.quantifier_map().len(), 1);
            let elt = TypeSymbol::declared("Elt", fixture.symbols).expect("a Type token");
            assert!(function.canonical_quantifier(elt).is_some());
        });
    });
}

#[test]
fn a_lambda_capturing_its_binder_is_a_node_of_its_knot() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = [(FN :{} -> Any = (a))]");
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let (a, Circular::List(list)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a list node");
            };
            assert_eq!(a.member().knot().len(), 2);
            let lambda = follow(a, list.cells()[0]);
            let function = lambda.function().expect("the item is a function node");
            let shape = activation.shape();
            assert!(std::ptr::eq(
                function.shape(),
                body_of(shape, item_lambda(shape, 0, 0))
            ));
            let any = fixture
                .types
                .function_type(fixture.scratch(), &[], &[], KType::ANY);
            assert_eq!(lambda.ktype(), any.handle);
            assert_eq!(list.ktype(), fixture.types.list(any.handle));
            assert!(follow(lambda, function.closure().get(CaptureSlot(0))) == a);
        });
    });
}

#[test]
fn a_lambda_capturing_a_fellow_function_is_a_node_of_the_knot() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = [(FN :{} -> Any = (f))]\nLET f = (FN :{} -> Any = (a))");
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let f = callable(fixture, activation, "f");
            let (a, Circular::List(list)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a list node");
            };
            assert_eq!(a.member().knot().len(), 3);
            let lambda = follow(a, list.cells()[0]);
            assert!(lambda != a && lambda != f);
            let function = lambda.function().expect("the item is a function node");
            assert!(follow(lambda, function.closure().get(CaptureSlot(0))) == f);
        });
    });
}

#[test]
fn a_lambda_below_a_nested_constructor_makes_its_path_anonymous() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = {inner = [(FN :{} -> Any = (a))] plain = [1 2]}");
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let (a, Circular::Record(record)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a record node");
            };
            assert_eq!(a.member().knot().len(), 3);
            let inner = follow(a, *record.field(fixture.name("inner").symbol()).unwrap());
            let (_, Circular::List(list)) = circular(Value::Knotted(inner)) else {
                panic!("`inner` is an anonymous list node");
            };
            let lambda = follow(inner, list.cells()[0]);
            let function = lambda.function().expect("the item is a function node");
            assert!(follow(lambda, function.closure().get(CaptureSlot(0))) == a);
            let Link::Value(Value::List(plain)) =
                record.field(fixture.name("plain").symbol()).unwrap()
            else {
                panic!("a constructor with no edge below it is an ordinary value");
            };
            assert_eq!(plain.len(), 2);
        });
    });
}

#[test]
fn a_lambda_part_capturing_no_fellow_is_asked_of_the_caller() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET k = 7\nLET a = [(FN :{} -> Number = (k)) f]\nLET f = (FN :{} -> Any = (a))",
        );
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &["a", "f"]);
            let part = item_lambda(activation.shape(), 1, 0);
            let mut asked = Vec::new();
            let refused = tie_with(fixture, writer, activation, "a", &mut |site, _| {
                asked.push(site);
                None
            })
            .err();
            assert_eq!(
                refused,
                Some(Untieable::Eager {
                    name: fixture.name("a"),
                    site: Site::of(part),
                })
            );
            assert_eq!(asked, [Site::of(part)]);
        });
    });
}
