//! The tie: a lone function, closure words, knots of fellow members read through their edges, and
//! every refusal.

use crate::elaborate::Elaboration;
use crate::scope::{Binding, CaptureSlot, Coordinate, Target};
use crate::type_lattice::KType;
use crate::values::Link;
use crate::values::{Knotted as _, Value, Weight};

use super::super::{KActivation, Knotted, Node, Untieable, tie};
use super::{Fixture, bound, callable, pin, read, with_fixture};

/// The component `name` belongs to, tied again — a refusal the runner left for the test to see.
fn tie_of<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    writer: crate::memory::Writer<'cell>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> Result<crate::memory::Knot<'cell, Node<'graph, 'cell>>, Untieable> {
    let shape = activation.shape();
    let (slot, _) = shape.slot(fixture.name(name)).unwrap();
    tie(
        writer,
        activation,
        shape.component_of(slot),
        fixture.types,
        fixture.scratch(),
    )
}

#[test]
fn a_lone_function_is_a_one_node_knot_typed_by_its_signature() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET f = (FN :{x :Number} -> Number = (x))");
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let f = callable(fixture, activation, "f");
            assert_eq!(f.member().knot().len(), 1);
            assert!(f.function().expect("a function").closure().is_empty());
            let x = fixture.name("x");
            let scratch = fixture.scratch();
            assert_eq!(
                f.ktype(),
                fixture
                    .types
                    .function_type(scratch, &[(x, KType::NUMBER)], KType::NUMBER)
            );
            assert!(std::ptr::eq(
                f.function().expect("a function").shape(),
                activation
                    .shape()
                    .births(activation.shape().slot(fixture.name("f")).unwrap().0)
                    .unwrap()
            ));
            // The knot's weight: its header, one node, and an empty closure run.
            assert_eq!(
                f.weight(),
                Weight::flat::<usize>()
                    .plus(Weight::flat::<Node<'static, 'static>>())
                    .plus(f.function().expect("a function").closure().weight())
            );
        });
    });
}

#[test]
fn a_closure_captures_the_enclosing_value_word() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = \"kept\"\nLET f = (FN :{} -> Str = (k))");
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let f = callable(fixture, activation, "f");
            let Link::Value(Value::Str(captured)) = f
                .function()
                .expect("a function")
                .closure()
                .get(CaptureSlot(0))
            else {
                panic!("`k` is captured as its value word");
            };
            let Value::Str(slot) = bound(fixture, activation, "k") else {
                panic!("`k` is a string");
            };
            assert!(std::ptr::eq(captured, slot), "the word, not a copy");
        });
    });
}

/// The coordinate `name`'s single mention in `callable`'s body reads through.
fn capture_read<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    writer: crate::memory::Writer<'cell>,
    callable: Knotted<'graph, 'cell>,
    builtins: &'cell crate::scope::Builtins<'graph, 'cell, Knotted<'graph, 'cell>>,
    name: &str,
) -> Binding<'graph, 'cell, Knotted<'graph, 'cell>> {
    let function = callable.function().expect("a function");
    let name = fixture.name(name);
    let mention = function
        .shape()
        .mentions()
        .iter()
        .find(|mention| mention.name == name)
        .expect("the body reads the name");
    assert!(matches!(
        mention.coordinate,
        Coordinate::Activation {
            hops: 0,
            target: Target::Capture(_)
        }
    ));
    let call = KActivation::of_callable(
        writer,
        function.shape(),
        callable,
        function.closure(),
        builtins,
    );
    call.read(mention.coordinate)
}

#[test]
fn mutual_recursion_is_one_knot_whose_edges_read_as_siblings() {
    with_fixture(|fixture| {
        let lines =
            fixture.parse("LET f = (FN :{} -> Number = (g))\nLET g = (FN :{} -> Number = (f))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let (f, g) = (
                callable(fixture, activation, "f"),
                callable(fixture, activation, "g"),
            );
            assert_eq!(f.member().knot().len(), 2);
            assert!(std::ptr::eq(
                f.member().knot().member(g.member().index()).payload(),
                g.node()
            ));
            let edge = |callable: Knotted<'_, '_>| match callable
                .function()
                .expect("a function")
                .closure()
                .get(CaptureSlot(0))
            {
                Link::Edge(edge) => edge.index(),
                Link::Value(_) => panic!("a fellow member is captured as an edge"),
            };
            assert_eq!(edge(f), g.member().index().index());
            assert_eq!(edge(g), f.member().index().index());
            let builtins = activation.builtins();
            let Binding::Bound(Value::Knotted(sibling)) =
                capture_read(fixture, writer, f, builtins, "g")
            else {
                panic!("an edge capture reads as the sibling callable");
            };
            assert!(std::ptr::eq(sibling.node(), g.node()));
        });
    });
}

#[test]
fn a_self_recursive_function_edges_itself() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET loop = (FN :{} -> Number = (loop))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let looped = callable(fixture, activation, "loop");
            assert_eq!(looped.member().knot().len(), 1);
            let Binding::Bound(Value::Knotted(itself)) =
                capture_read(fixture, writer, looped, activation.builtins(), "loop")
            else {
                panic!("a self capture reads as the callable itself");
            };
            assert!(std::ptr::eq(itself.node(), looped.node()));
        });
    });
}

#[test]
fn a_nested_capture_of_an_enclosing_edge_reads_the_sibling_value() {
    with_fixture(|fixture| {
        let lines = fixture
            .parse("LET f = (FN :{} -> Number = (\n    LET h = (FN :{} -> Number = (f))\n    h))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let f = callable(fixture, activation, "f");
            let function = f.function().expect("a function");
            let call = crate::memory::resident(
                writer,
                KActivation::of_callable(
                    writer,
                    function.shape(),
                    f,
                    function.closure(),
                    activation.builtins(),
                ),
            );
            let knot = tie_of(fixture, writer, call, "h").expect("the inner function ties");
            let h = Knotted::of(knot, 0);
            let Link::Value(Value::Knotted(captured)) = h
                .function()
                .expect("a function")
                .closure()
                .get(CaptureSlot(0))
            else {
                panic!("a capture of the enclosing knot's edge is the sibling's value");
            };
            assert!(std::ptr::eq(captured.node(), f.node()));
        });
    });
}

#[test]
fn a_pending_capture_refuses_with_its_binder() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = 3\nLET f = (FN :{} -> Number = (k))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["k", "f"]);
            assert!(matches!(
                read(fixture, activation, "k"),
                Binding::Pending(_)
            ));
            assert_eq!(
                tie_of(fixture, writer, activation, "f").err(),
                Some(Untieable::Pending {
                    name: fixture.name("k"),
                    binder,
                })
            );
        });
    });
}

#[test]
fn a_pending_signature_type_refuses_with_its_binder() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET Alias = Str\nLET f = (FN :{x :Alias} -> Number = (1))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["Alias", "f"]);
            assert_eq!(
                tie_of(fixture, writer, activation, "f").err(),
                Some(Untieable::Pending {
                    name: fixture.name("Alias"),
                    binder,
                })
            );
        });
    });
}

#[test]
fn a_component_with_a_data_member_is_untieable() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = [f]\nLET f = (FN :{} -> Number = (a))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["a"]);
            assert_eq!(
                tie_of(fixture, writer, activation, "f").err(),
                Some(Untieable::Data {
                    name: fixture.name("a"),
                })
            );
        });
    });
}

#[test]
fn an_unsupported_signature_is_a_type_refusal() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET f = (FN :{x :(Number AS Any)} -> Number = (x))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["f"]);
            assert!(matches!(
                tie_of(fixture, writer, activation, "f"),
                Err(Untieable::Type(Elaboration::Unsupported { .. }))
            ));
        });
    });
}
