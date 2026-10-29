//! List views: slices and concatenations over flat runs, read, weighed, typed, copied and
//! compared as the flat lists they stand for.
//!
//! `a_copied_view_outlives_its_sources` is on the Miri slate: a deep copy reading a view's
//! stretches out of a region released after.

use proptest::prelude::*;

use crate::memory::{CellGraph, ReleaseAbsorption, Writer};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::view::VIEW_STEPS;
use crate::values::{Chain, Piece, ValueFamily, satisfies};

use super::{List, Record, TestGraph, Value, copy, parts, pin, text, tie, with_fixture};

/// A list of the strings `"0"`, `"1"`, … `"n-1"`, flat: distinct elements of varying weights.
fn numerals<'cell>(
    writer: Writer<'cell, '_>,
    n: usize,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: crate::memory::BumpAllocator<'_>,
) -> &'cell List<'cell> {
    let items: Vec<_> = (0..n).map(|i| text(writer, &i.to_string())).collect();
    List::new(writer, items.into_iter(), types, scratch)
}

/// The numerals a list of them holds, in order, read through the door and through its elements.
fn read(
    list: &List<'_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: crate::memory::BumpAllocator<'_>,
) -> Vec<usize> {
    let numeral = |value: Value<'_>| value.as_str().unwrap().parse::<usize>().unwrap();
    let through_door: Vec<_> = parts(Value::List(list), types, scratch)
        .into_iter()
        .map(numeral)
        .collect();
    let walked: Vec<_> = list.elements().map(numeral).collect();
    assert_eq!(through_door, walked, "a get and a walk read alike");
    assert_eq!(list.len(), walked.len());
    walked
}

/// One edit of the tree-law program: bounds are taken modulo the current length plus one.
#[derive(Clone, Debug)]
enum Edit {
    Slice(usize, usize),
    Backward(usize, usize),
    /// The base's elements `[a, b)`, laid flat after the current list.
    Append(usize, usize),
    /// A slice of the base, spread before the current list.
    Prepend(usize, usize),
}

fn edit() -> impl Strategy<Value = Edit> {
    let bounds = (0..64usize, 0..64usize);
    prop_oneof![
        bounds.clone().prop_map(|(a, b)| Edit::Slice(a, b)),
        bounds.clone().prop_map(|(a, b)| Edit::Backward(a, b)),
        bounds.clone().prop_map(|(a, b)| Edit::Append(a, b)),
        bounds.prop_map(|(a, b)| Edit::Prepend(a, b)),
    ]
}

/// `(a, b)` modulo `len + 1`, ordered.
fn range(a: usize, b: usize, len: usize) -> (usize, usize) {
    let (a, b) = (a % (len + 1), b % (len + 1));
    (a.min(b), a.max(b))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any run of slices, backward slices and concatenations reads, element by element, as a
    /// model vector does; every tree stays balanced with its memos exact; and every view weighs
    /// what a flat list of its elements weighs.
    #[test]
    fn a_view_reads_as_its_model_and_keeps_its_laws(n in 0..48usize, edits in prop::collection::vec(edit(), 1..20)) {
        with_fixture(|fixture| {
            fixture.in_cell(pin, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let scratch = fixture.scratch();
                let base = numerals(writer, n, types, scratch);
                let (mut current, mut model): (&List<'_>, Vec<usize>) = (base, (0..n).collect());
                for edit in &edits {
                    let len = model.len();
                    current = match *edit {
                        Edit::Slice(a, b) => {
                            let (from, to) = range(a, b, len);
                            model = model[from..to].to_vec();
                            List::sliced(writer, Value::List(current), (from, to), false, types)
                        }
                        Edit::Backward(a, b) => {
                            let (from, to) = range(a, b, len);
                            model = model[from..to].iter().rev().copied().collect();
                            List::sliced(writer, Value::List(current), (from, to), true, types)
                        }
                        Edit::Append(a, b) => {
                            let (from, to) = range(a, b, n);
                            model.extend(from..to);
                            let items: Vec<_> = base.cells()[from..to].to_vec();
                            let pieces = [Piece::Spread(Value::List(current)), Piece::Items(&items)];
                            List::concatenated(writer, &pieces, types, scratch)
                        }
                        Edit::Prepend(a, b) => {
                            let (from, to) = range(a, b, n);
                            model = (from..to).chain(model.iter().copied()).collect();
                            let slice = List::sliced(writer, Value::List(base), (from, to), false, types);
                            let pieces = [Piece::Spread(Value::List(slice)), Piece::Spread(Value::List(current))];
                            List::concatenated(writer, &pieces, types, scratch)
                        }
                    };
                    assert_eq!(read(current, types, scratch), model);
                    assert!(current.holds_its_laws());
                    let flat = List::new(writer, current.elements(), types, scratch);
                    assert_eq!(current.weight(), flat.weight(), "a view weighs what its copy writes");
                }
            })
        });
    }
}

/// The view steps `run` takes.
fn steps(run: impl FnOnce()) -> usize {
    let before = VIEW_STEPS.with(|steps| steps.get());
    run();
    VIEW_STEPS.with(|steps| steps.get()) - before
}

#[test]
fn a_read_and_an_edit_of_a_view_built_by_n_edits_cost_log_n_steps() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let scratch = fixture.scratch();
            // Each edit inserts one element at a scattered place: two splits and two joins.
            let mut current = numerals(writer, 1, types, scratch);
            let (mut edits, mut reads) = (Vec::new(), Vec::new());
            for n in 1..=1usize << 12 {
                let item = [Value::Number(n as f64)];
                let mid = n * 7919 % (current.len() + 1);
                let edit = steps(|| {
                    let source = Value::List(current);
                    let len = current.len();
                    let pieces = [
                        Piece::Spread(Value::List(List::sliced(
                            writer,
                            source,
                            (0, mid),
                            false,
                            types,
                        ))),
                        Piece::Items(&item),
                        Piece::Spread(Value::List(List::sliced(
                            writer,
                            source,
                            (mid, len),
                            false,
                            types,
                        ))),
                    ];
                    current = List::concatenated(writer, &pieces, types, scratch);
                });
                if n.is_power_of_two() && n >= 1 << 8 {
                    edits.push(edit);
                    reads.push(steps(|| {
                        current.get(current.len() / 3);
                    }));
                }
            }
            assert!(current.holds_its_laws());
            assert_eq!(
                steps(|| assert!(current.len() > 4096)),
                0,
                "a view counts by its memo"
            );
            // Each doubling of n adds at most a constant: a tree one or two levels taller.
            for (what, counts, most) in [("edit", &edits, 24), ("read", &reads, 3)] {
                for pair in counts.windows(2) {
                    assert!(
                        pair[1] <= pair[0] + most,
                        "{what} steps grow faster than log n: {counts:?}"
                    );
                }
            }
        })
    });
}

#[test]
fn a_slice_weighs_its_elements_whatever_its_source_holds() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let scratch = fixture.scratch();
            for n in [15, 16, 17, 33, 40] {
                let source = numerals(writer, n, types, scratch);
                for from in 0..=n {
                    for to in from..=n {
                        for backward in [false, true] {
                            let slice = List::sliced(
                                writer,
                                Value::List(source),
                                (from, to),
                                backward,
                                types,
                            );
                            let mut cells = source.cells()[from..to].to_vec();
                            if backward {
                                cells.reverse();
                            }
                            let flat = List::new(writer, cells.into_iter(), types, scratch);
                            assert_eq!(slice.weight(), flat.weight(), "n {n}, [{from}, {to})");
                        }
                    }
                }
            }
        })
    });
}

#[test]
fn a_slice_of_a_data_node_reads_its_links_through_their_holder() {
    use super::{Holding, Link, Node};
    type Linked<'cell> = crate::values::List<'cell, Node<'cell>, Link<'cell>>;
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let memo = types.list(KType::ANY);
            let nodes = tie(writer, 1, |_, edges| {
                let edge = Link::Edge(edges[0]);
                let mut cells = vec![edge];
                cells.extend(
                    (0..20).map(|i| Link::Value(crate::values::text(writer, &i.to_string()))),
                );
                crate::values::Circular::List(Linked::linked(writer, &cells, memo))
            });
            let source = Holding::Knotted(nodes[0]);
            let slice = crate::values::List::sliced(writer, source, (0, 18), true, types);
            let elements: Vec<_> = slice.elements().collect();
            assert_eq!(elements.len(), 18);
            assert_eq!(elements[0].as_str(), Some("16"));
            assert!(matches!(elements[17], Holding::Knotted(node) if node == nodes[0]));
            // The fixture's knot weighs nothing, so its edge weighs its link word alone.
            let link = crate::values::Weight::flat::<Link<'static>>();
            let words = (0..17).fold(crate::values::Weight::ZERO, |sum, i: usize| {
                sum.plus(link)
                    .plus(crate::values::Weight::text(i.to_string().len()))
            });
            assert_eq!(
                slice.weight(),
                crate::values::Weight::flat::<crate::values::List<'static, Node<'static>>>()
                    .plus(link)
                    .plus(words)
                    .plus(crate::values::Weight::run::<crate::values::view::Prefix>(2))
            );
        })
    });
}

#[test]
fn a_view_equals_renders_digests_and_satisfies_as_its_flat_copy() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
            let items = [Value::Number(1.0), text(writer, "a"), Value::Number(2.0)];
            let source = Value::List(List::new(writer, items.into_iter(), types, scratch));
            let view = Value::List(List::sliced(writer, source, (0, 3), true, types));
            let reversed = [Value::Number(2.0), text(writer, "a"), Value::Number(1.0)];
            let flat = List::new(writer, reversed.into_iter(), types, scratch);
            let flat = Value::List(flat);
            assert_eq!(view.equals(&flat, types, scratch), Ok(true));
            assert_eq!(view.equals(&source, types, scratch), Ok(false));
            let render = |value: Value<'_>| {
                let mut out = String::new();
                value.render(&mut out, types, symbols, scratch).unwrap();
                out
            };
            assert_eq!(render(view), render(flat));
            assert_eq!(view.digest(types, scratch), flat.digest(types, scratch));
            for slot in [
                types.list(KType::NUMBER),
                view.concrete_ktype(),
                KType::LIST_OF_ANY,
            ] {
                assert_eq!(
                    satisfies(slot, &view, types, scratch),
                    satisfies(slot, &flat, types, scratch)
                );
            }
        })
    });
}

#[test]
fn a_slice_carries_its_source_s_type_and_a_concatenation_the_union_of_its_sources() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let scratch = fixture.scratch();
            let items = [Value::Number(1.0), text(writer, "a")];
            let mixed = Value::List(List::new(writer, items.into_iter(), types, scratch));
            let slice = Value::List(List::sliced(writer, mixed, (0, 1), false, types));
            assert_eq!(slice.concrete_ktype(), mixed.concrete_ktype());
            assert!(!satisfies(
                types.list(KType::NUMBER),
                &slice,
                types,
                scratch
            ));

            let numbers = Value::List(List::new(
                writer,
                [Value::Number(1.0)].into_iter(),
                types,
                scratch,
            ));
            let strings = Value::List(List::new(
                writer,
                [text(writer, "b")].into_iter(),
                types,
                scratch,
            ));
            let both = [Piece::Spread(numbers), Piece::Spread(strings)];
            let both = List::concatenated(writer, &both, types, scratch);
            assert_eq!(both.ktype(), mixed.concrete_ktype());
            let twice = [Piece::Spread(numbers), Piece::Spread(numbers)];
            let twice = List::concatenated(writer, &twice, types, scratch);
            assert_eq!(twice.ktype(), types.list(KType::NUMBER));
            assert!(twice.is_view());
            let empty = List::concatenated(writer, &[Piece::Items(&[])], types, scratch);
            assert!(!empty.is_view());
            assert_eq!(empty.ktype(), types.list(KType::NEVER));
        })
    });
}

#[test]
fn a_copied_view_outlives_its_sources() {
    with_fixture(|fixture| {
        let scratch = fixture.scratch();
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let source = numerals(writer, 40, types, scratch);
                let slice = List::sliced(writer, Value::List(source), (3, 30), true, types);
                let pieces = [
                    Piece::Spread(Value::List(slice)),
                    Piece::Spread(Value::List(source)),
                ];
                let view = List::concatenated(writer, &pieces, types, scratch);
                let carrier = context.lift::<ValueFamily>(Value::List(view));
                let crossed = fixture.tiers.cross(context, dest, &carrier).unwrap();
                let Value::List(copied) = context.read(&crossed).value() else {
                    panic!("a list crosses as a list");
                };
                assert!(!copied.is_view(), "a copy lays a view flat");
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let types = &fixture.tiers.of(context);
                let carrier = context.redeem(dormant).unwrap();
                let Value::List(copied) = context.read(&carrier).value() else {
                    panic!("the kept list redeems as a list");
                };
                let expected: Vec<usize> = (3..30).rev().chain(0..40).collect();
                assert_eq!(read(copied, types, scratch), expected);
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_pinned_view_stays_a_view() {
    with_fixture(|fixture| {
        let scratch = fixture.scratch();
        let mut graph: TestGraph<'_> = CellGraph::new(2, pin);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let source = numerals(writer, 8, types, scratch);
                let view = List::sliced(writer, Value::List(source), (1, 5), false, types);
                let carrier = context.lift::<ValueFamily>(Value::List(view));
                let crossed = fixture.tiers.cross(context, dest, &carrier).unwrap();
                let Value::List(pinned) = context.read(&crossed).value() else {
                    panic!("a list crosses as a list");
                };
                assert!(pinned.is_view());
                assert!(pinned.same_run(view));
            })
            .unwrap();
    });
}

#[test]
fn a_concatenation_copies_a_source_that_hides_a_field_its_type_would_show() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
            let name = |text| BinderSymbol::declared(text, symbols).unwrap();
            let (x, y) = (name("x"), name("y"));
            let fields = [(x, Value::Number(1.0)), (y, Value::Number(2.0))];
            let record = Value::Record(Record::new(writer, &fields, types, scratch));
            let narrow = types.list(types.record(scratch, &[(x, KType::NUMBER)]));
            let source = Value::List(List::new(writer, [record].into_iter(), types, scratch));
            let source = source.retyped(writer, narrow, types, scratch);
            assert!(source.as_list().unwrap().weight().hides());
            let other = [Value::Record(Record::new(writer, &fields, types, scratch))];
            let pieces = [Piece::Spread(source), Piece::Items(&other)];
            let both = List::concatenated(writer, &pieces, types, scratch);
            let Value::Record(copied) = parts(Value::List(both), types, scratch)[0] else {
                panic!("a record element");
            };
            assert_eq!(copied.names().len(), 1, "the copy stores `x` alone");
            assert!(!both.weight().hides(), "the copy hides nothing");
            // A source seen at the new element type as it was keeps its run.
            let alone = List::concatenated(writer, &[Piece::Spread(source)], types, scratch);
            assert!(alone.weight().hides());
            assert!(alone.is_view());
        })
    });
}

/// One piece of a knot's list in the knot-law program: bounds are taken modulo the base's length
/// plus one.
#[derive(Clone, Debug)]
enum KnotPiece {
    /// The base's numerals `[a, b)` as the list's own cells, then an edge to node 0 where set.
    Items(usize, usize, bool),
    /// A slice of the base, spread.
    Spread(usize, usize),
    /// Node 0, a flat data node of the same knot, spread.
    Fellow,
}

fn knot_piece() -> impl Strategy<Value = KnotPiece> {
    let bounds = (0..64usize, 0..64usize);
    prop_oneof![
        (bounds.clone(), any::<bool>()).prop_map(|((a, b), edge)| KnotPiece::Items(a, b, edge)),
        bounds.prop_map(|(a, b)| KnotPiece::Spread(a, b)),
        Just(KnotPiece::Fellow),
    ]
}

/// What a knot list's element is in the model: a numeral, or the knot's node at an index.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    Numeral(usize),
    Node(usize),
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// A knot's list built from its own cells, plain spreads and a fellow node reads as its model
    /// through its member, its links keeping its own edges; it keeps its laws and weighs what its
    /// flat copy writes. A slice of it, and a list spreading it, read its edges as the knot's
    /// nodes, keep their laws and weigh what their flat copies write.
    #[test]
    fn a_knot_s_list_view_reads_as_its_model_and_keeps_its_laws(
        n in 0..40usize,
        pieces in prop::collection::vec(knot_piece(), 1..8),
        cut in (0..64usize, 0..64usize, any::<bool>()),
    ) {
        use super::{Holding, Link, Node};
        use crate::values::{Circular, LinkedPiece};
        type Linked<'cell> = crate::values::List<'cell, Node<'cell>, Link<'cell>>;
        with_fixture(|fixture| {
            fixture.in_cell(pin, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let scratch = fixture.scratch();
                let memo = types.list(KType::ANY);
                let plain: Vec<Holding<'_>> =
                    (0..n).map(|i| crate::values::text(writer, &i.to_string())).collect();
                let base: Vec<_> = plain.iter().map(|value| Link::Value(*value)).collect();
                let numeral = |value: Holding<'_>| value.as_str().unwrap().parse::<usize>().unwrap();
                let fellow_model = [Held::Node(1), Held::Numeral(100), Held::Numeral(101)];
                let mut model = Vec::new();
                let mut fellow: Option<&Linked<'_>> = None;
                let nodes = tie(writer, 2, |index, edges| {
                    if index == 0 {
                        let cells = [
                            Link::Edge(edges[1]),
                            Link::Value(crate::values::text(writer, "100")),
                            Link::Value(crate::values::text(writer, "101")),
                        ];
                        let list = Linked::linked(writer, &cells, memo);
                        fellow = Some(list);
                        return Circular::List(list);
                    }
                    let mut owned = Vec::new();
                    for piece in &pieces {
                        if let KnotPiece::Items(a, b, edge) = *piece {
                            let (from, to) = range(a, b, n);
                            let mut cells = base[from..to].to_vec();
                            if edge {
                                cells.push(Link::Edge(edges[0]));
                            }
                            owned.push(cells);
                        }
                    }
                    let mut owned = owned.iter();
                    let mut built = Vec::new();
                    for piece in &pieces {
                        built.push(match *piece {
                            KnotPiece::Items(a, b, edge) => {
                                let (from, to) = range(a, b, n);
                                model.extend((from..to).map(Held::Numeral));
                                if edge {
                                    model.push(Held::Node(0));
                                }
                                LinkedPiece::Items(owned.next().unwrap())
                            }
                            KnotPiece::Spread(a, b) => {
                                let (from, to) = range(a, b, n);
                                model.extend((from..to).map(Held::Numeral));
                                let source = crate::values::List::new(writer, plain.iter().copied(), types, scratch);
                                let slice = crate::values::List::sliced(writer, Holding::List(source), (from, to), false, types);
                                LinkedPiece::Spread(Holding::List(slice))
                            }
                            KnotPiece::Fellow => {
                                model.extend(fellow_model);
                                LinkedPiece::Fellow(fellow.unwrap(), false)
                            }
                        });
                    }
                    Circular::List(Linked::linked_pieces(writer, &built, memo, types, scratch).0)
                });
                let as_held = |value: Holding<'_>| match value {
                    Holding::Knotted(node) => Held::Node(nodes.iter().position(|n| *n == node).unwrap()),
                    value => Held::Numeral(numeral(value)),
                };
                let Some((_, Circular::List(list))) = Holding::Knotted(nodes[1]).as_circular() else {
                    panic!("node 1 is a list")
                };
                assert!(list.holds_its_laws());
                let alone = matches!(pieces.as_slice(), [KnotPiece::Items(..)]);
                // A list that spreads only nothing is the flat empty list.
                assert_eq!(
                    list.is_view(),
                    !alone && !model.is_empty(),
                    "a list that spreads is a view"
                );
                let links: Vec<_> = list.links().collect();
                let read: Vec<_> = links.iter().map(|link| match link {
                    Link::Edge(edge) => Held::Node(edge.index() as usize),
                    Link::Value(value) => Held::Numeral(numeral(*value)),
                }).collect();
                assert_eq!(read, model, "its links in order");
                let through_door: Vec<_> = parts(Holding::Knotted(nodes[1]), types, scratch).into_iter().map(as_held).collect();
                assert_eq!(through_door, model, "read through its member");
                assert_eq!(list.weight(), Linked::linked(writer, &links, memo).weight(), "it weighs what its flat copy writes");

                let (from, to) = range(cut.0, cut.1, model.len());
                let slice = crate::values::List::sliced(writer, Holding::Knotted(nodes[1]), (from, to), cut.2, types);
                let mut expected = model[from..to].to_vec();
                if cut.2 {
                    expected.reverse();
                }
                let elements: Vec<_> = slice.elements().map(as_held).collect();
                assert_eq!(elements, expected, "a slice reads its edges as the knot's nodes");
                assert!(slice.holds_its_laws());
                let flat = crate::values::List::new(writer, slice.elements(), types, scratch);
                assert_eq!(slice.weight(), flat.weight());

                let spread = [Piece::Spread(Holding::List(slice)), Piece::Spread(Holding::Knotted(nodes[1]))];
                let both = crate::values::List::concatenated(writer, &spread, types, scratch);
                let elements: Vec<_> = both.elements().map(as_held).collect();
                assert_eq!(elements, [expected, model.clone()].concat());
                assert!(both.holds_its_laws());
                let flat = crate::values::List::new(writer, both.elements(), types, scratch);
                assert_eq!(both.weight(), flat.weight());
            })
        });
    }
}

#[test]
fn a_knot_s_list_spreading_a_long_list_lays_a_constant_number_of_nodes() {
    use super::{Holding, Link, Node};
    use crate::values::{Circular, LinkedPiece};
    type Linked<'cell> = crate::values::List<'cell, Node<'cell>, Link<'cell>>;
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let scratch = fixture.scratch();
            let memo = types.list(KType::ANY);
            let laid = |n: usize| {
                let items: Vec<Holding<'_>> = (0..n)
                    .map(|i| crate::values::text(writer, &i.to_string()))
                    .collect();
                let source = crate::values::List::new(writer, items.into_iter(), types, scratch);
                let mut built = None;
                let laid = steps(|| {
                    tie(writer, 1, |_, edges| {
                        let own = [Link::Edge(edges[0])];
                        let pieces = [
                            LinkedPiece::Spread(Holding::List(source)),
                            LinkedPiece::Items(&own),
                        ];
                        let list = Linked::linked_pieces(writer, &pieces, memo, types, scratch).0;
                        built = Some(list);
                        Circular::List(list)
                    });
                });
                let list = built.unwrap();
                assert!(list.is_view() && list.len() == n + 1);
                laid
            };
            assert_eq!(laid(16), laid(4096), "a spread lays no cell of its source");
        })
    });
}
