//! The tie: a lone function, closure words, knots of fellow members read through their edges, data
//! members and the anonymous nodes below them, and every refusal.

use crate::elaborate::Elaboration;
use crate::memory::{Knot, Writer};
use crate::parse::ExpressionPart;
use crate::scope::{Binding, CaptureSlot, Coordinate, Site, Target};
use crate::type_lattice::{KType, NodeSchema, TypeNode};
use crate::values::{Circular, ConstructionRefused, KeyRejected, Link};
use crate::values::{Knotted as _, Value, Weight};

use super::super::{KActivation, Knotted, Node, Supplied, Untieable, tie};
use super::{Fixture, bound, callable, circular, declared, follow, pin, read, with_fixture};

/// The component `name` belongs to, tied again with `eager` — a refusal the runner left for the
/// test to see.
fn tie_with<'f, 'graph, 'cell>(
    fixture: &'f Fixture<'_, 'graph>,
    writer: Writer<'cell>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
    eager: &mut dyn FnMut(Site) -> Option<Supplied<'graph, 'cell>>,
) -> Result<Knot<'cell, Node<'graph, 'cell>>, Untieable<'f>> {
    let shape = activation.shape();
    let (slot, _) = shape.slot(fixture.name(name)).unwrap();
    tie(
        writer,
        activation,
        shape.component_of(slot),
        fixture.types,
        fixture.scratch(),
        eager,
    )
}

/// [`tie_with`] under an evaluator that supplies nothing.
fn tie_of<'f, 'graph, 'cell>(
    fixture: &'f Fixture<'_, 'graph>,
    writer: Writer<'cell>,
    activation: &KActivation<'graph, 'cell>,
    name: &str,
) -> Result<Knot<'cell, Node<'graph, 'cell>>, Untieable<'f>> {
    tie_with(fixture, writer, activation, name, &mut |_| None)
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

#[test]
fn a_tagged_self_reference_is_a_two_node_knot() {
    with_fixture(|fixture| {
        let lines = fixture.parse("NEWTYPE Ring = :{next :Ring}\nLET a = (Ring {next = a})");
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let ring = declared(fixture, activation, "Ring");
            let (a, Circular::Tagged(tagged)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a tagged node");
            };
            assert_eq!(a.member().index().index(), 0);
            assert_eq!(a.member().knot().len(), 2);
            assert_eq!(tagged.ktype(), ring);
            let record = follow(a, *tagged.payload());
            assert_eq!(record.member().index().index(), 1);
            let (_, Circular::Record(fields)) = circular(Value::Knotted(record)) else {
                panic!("the payload is an anonymous record node");
            };
            let next = fixture.name("next");
            assert_eq!(
                fields.ktype(),
                fixture.types.record(fixture.scratch(), &[(next, ring)])
            );
            let link = *fields.field(next.symbol()).expect("a `next` field");
            assert!(follow(record, link) == a);
        });
    });
}

#[test]
fn a_tagged_ring_of_two_members_is_one_knot() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "NEWTYPE Ring = :{next :Ring}\nLET a = (Ring {next = b})\nLET b = (Ring {next = a})",
        );
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let next = fixture.name("next").symbol();
            fn successor<'graph, 'cell>(
                member: Knotted<'graph, 'cell>,
                next: crate::parse::Symbol,
            ) -> Knotted<'graph, 'cell> {
                let (_, Circular::Tagged(tagged)) = circular(Value::Knotted(member)) else {
                    panic!("a ring member is tagged");
                };
                let record = follow(member, *tagged.payload());
                let (_, Circular::Record(fields)) = circular(Value::Knotted(record)) else {
                    panic!("its payload is a record node");
                };
                follow(record, *fields.field(next).unwrap())
            }
            let (a, _) = circular(bound(fixture, activation, "a"));
            let (b, _) = circular(bound(fixture, activation, "b"));
            assert_eq!(a.member().knot().len(), 4);
            assert!(a.member().knot() == b.member().knot());
            assert!(successor(a, next) == b && successor(b, next) == a);
        });
    });
}

#[test]
fn a_container_and_the_function_that_captures_it_share_a_knot() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = [f]\nLET f = (FN :{} -> Any = (a))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let f = callable(fixture, activation, "f");
            let (a, Circular::List(list)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a list node");
            };
            assert!(a.member().knot() == f.member().knot());
            assert!(follow(a, list.cells()[0]) == f);
            assert_eq!(list.ktype(), fixture.types.list(f.ktype()));
            let Link::Edge(edge) = f
                .function()
                .expect("a function")
                .closure()
                .get(CaptureSlot(0))
            else {
                panic!("`f` captures `a` as an edge");
            };
            assert_eq!(edge, a.member().index());
            let Binding::Bound(read) = capture_read(fixture, writer, f, activation.builtins(), "a")
            else {
                panic!("the capture reads bound");
            };
            assert!(read.as_circular().is_some_and(|(read, _)| read == a));
        });
    });
}

#[test]
fn a_nested_constructor_on_a_sibling_path_is_an_anonymous_node() {
    with_fixture(|fixture| {
        let lines =
            fixture.parse("LET a = {inner = [f] plain = [1 2]}\nLET f = (FN :{} -> Any = (a))");
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let f = callable(fixture, activation, "f");
            let (a, Circular::Record(record)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a record node");
            };
            assert_eq!(a.member().knot().len(), 3);
            let inner = follow(a, *record.field(fixture.name("inner").symbol()).unwrap());
            assert_eq!(
                inner.member().index().index(),
                2,
                "anonymous nodes follow the members"
            );
            let (_, Circular::List(list)) = circular(Value::Knotted(inner)) else {
                panic!("`inner` is an anonymous list node");
            };
            assert!(follow(inner, list.cells()[0]) == f);
            let Link::Value(Value::List(plain)) =
                record.field(fixture.name("plain").symbol()).unwrap()
            else {
                panic!("a constructor with no sibling below it is an ordinary value");
            };
            assert_eq!(plain.len(), 2);
        });
    });
}

#[test]
fn a_cycle_of_containers_refuses_naming_it() {
    with_fixture(|fixture| {
        for (source, names) in [
            ("LET a = [1 a]", &["a"][..]),
            ("LET a = [b]\nLET b = {x = a}", &["a", "b"]),
        ] {
            let lines = fixture.parse(source);
            fixture.in_cell(pin, |context, binder| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, binder, names);
                let shape = activation.shape();
                let (slot, _) = shape.slot(fixture.name("a")).unwrap();
                let expected: Vec<_> = shape
                    .component_of(slot)
                    .members
                    .iter()
                    .map(|slot| shape.slot_name(*slot))
                    .collect();
                let Err(Untieable::TypeCycle { names: refused }) =
                    tie_of(fixture, writer, activation, "a")
                else {
                    panic!("`{source}` has no finite type");
                };
                assert_eq!(refused, expected.as_slice());
            });
        }
    });
}

#[test]
fn a_construction_the_rule_refuses_refuses_the_tie() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "NEWTYPE Ring = :{next :Ring}\nLET a = (Ring {other = a})\nLET b = (Number {next = b})",
        );
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["a", "b"]);
            let ring = declared(fixture, activation, "Ring");
            assert!(matches!(
                tie_of(fixture, writer, activation, "a"),
                Err(Untieable::Construction {
                    refused: ConstructionRefused::Misfit { identity, .. },
                    ..
                }) if identity == ring
            ));
            assert!(matches!(
                tie_of(fixture, writer, activation, "b"),
                Err(Untieable::Construction {
                    refused: ConstructionRefused::NotNewType(KType::NUMBER),
                    ..
                })
            ));
        });
    });
}

#[test]
fn a_nested_construction_is_built_through_the_checked_door() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "NEWTYPE Distance = Number\n\
             LET a = [(Distance 3) f]\nLET f = (FN :{} -> Any = (a))\n\
             LET b = [(Distance \"x\") g]\nLET g = (FN :{} -> Any = (b))",
        );
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["b"]);
            let distance = declared(fixture, activation, "Distance");
            let (_, Circular::List(list)) = circular(bound(fixture, activation, "a")) else {
                panic!("`a` is a list node");
            };
            let Link::Value(Value::Tagged(tagged)) = list.cells()[0] else {
                panic!("the construction is an ordinary tagged value");
            };
            assert_eq!(tagged.ktype(), distance);
            assert!(matches!(
                tie_of(fixture, writer, activation, "b"),
                Err(Untieable::Construction {
                    refused: ConstructionRefused::Misfit { .. },
                    ..
                })
            ));
        });
    });
}

#[test]
fn a_deferred_read_of_a_pending_binding_refuses() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = 3\nLET a = [k f]\nLET f = (FN :{} -> Any = (a))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["k", "a"]);
            assert_eq!(
                tie_of(fixture, writer, activation, "a").err(),
                Some(Untieable::Pending {
                    name: fixture.name("k"),
                    binder,
                })
            );
        });
    });
}

#[test]
fn an_eager_part_refuses_by_site_and_ties_when_supplied() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET g = (FN :{x :Number} -> Any = (x))\nLET a = [(g 1) f]\nLET f = (FN :{} -> Any = (a))",
        );
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["a"]);
            let ExpressionPart::ListLiteral(items) = lines[1].statement_spine().parts[3].value
            else {
                panic!("`a` is a list literal");
            };
            let call = Site::of(&items[0]);
            assert_eq!(
                tie_of(fixture, writer, activation, "a").err(),
                Some(Untieable::Eager {
                    name: fixture.name("a"),
                    site: call,
                })
            );
            let knot = tie_with(fixture, writer, activation, "a", &mut |site| {
                (site == call).then_some(Supplied::Value(Value::Number(7.0)))
            })
            .expect("the supplied part ties");
            let shape = activation.shape();
            let (slot, _) = shape.slot(fixture.name("a")).unwrap();
            let index = shape
                .component_of(slot)
                .members
                .binary_search(&slot)
                .unwrap();
            let (_, Circular::List(list)) = circular(Value::Knotted(Knotted::of(knot, index)))
            else {
                panic!("`a` is a list node");
            };
            assert!(matches!(list.cells()[0], Link::Value(Value::Number(7.0))));
        });
    });
}

#[test]
fn a_dict_key_that_is_no_scalar_refuses() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = [1]\nLET a = {(k): f}\nLET f = (FN :{} -> Any = (a))");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["a"]);
            let list = bound(fixture, activation, "k");
            let refused = tie_with(fixture, writer, activation, "a", &mut |_| {
                Some(Supplied::Value(list))
            });
            assert!(matches!(
                refused,
                Err(Untieable::Key {
                    rejected: KeyRejected::NotAScalar(_),
                    ..
                })
            ));
        });
    });
}

#[test]
fn a_type_declaration_is_declared_rather_than_tied() {
    with_fixture(|fixture| {
        let lines = fixture.parse("NEWTYPE Ring = :{next :Ring}");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let ring = declared(fixture, activation, "Ring");
            let next = fixture.name("next");
            let TypeNode::SetMember {
                schema: NodeSchema::NewType(representation),
                ..
            } = fixture.types.node(ring)
            else {
                panic!("a NEWTYPE binds a newtype member");
            };
            assert_eq!(
                representation,
                fixture.types.record(fixture.scratch(), &[(next, ring)]),
                "the door sealed the group, so the field reads its own binder"
            );
        });
    });
}

#[test]
fn a_type_binder_over_a_container_literal_is_opaque() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET Wrap = [1]");
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &["Wrap"]);
            assert_eq!(
                tie_of(fixture, writer, activation, "Wrap").err(),
                Some(Untieable::Opaque {
                    name: fixture.name("Wrap"),
                })
            );
        });
    });
}
