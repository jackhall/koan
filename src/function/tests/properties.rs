//! The law, over `scope`'s generated shape plans: a component of value binders that is cyclic or
//! births only callables ties exactly when the reads between its data members are acyclic, to one
//! knot whose closures follow its members' capture layouts and whose data nodes hold one link per
//! planned read — an edge for a fellow member, the enclosing word otherwise, and `null` for a part
//! the caller evaluates — and refuses naming data members otherwise; a copy of every knot is the
//! same knot rebuilt.
//!
//! The plans render a data binder of a cyclic component as a list literal: its deferred reads as
//! name tokens, and a lambda or a call over its eager reads as parenthesized items. The law reads
//! the planned reads back from that rendering through the shape's mentions.

use std::ptr;

use proptest::prelude::*;

use crate::memory::{CellGraph, Edge, ReleaseAbsorption, Writer, resident};
use crate::parse::{BinderSymbol, ExpressionPart};
use crate::scope::{
    Binding, CaptureSlot, CaptureSource, Component, Coordinate, Shape, ShapeKind, Site, Slot,
    Target,
};
use crate::type_lattice::KType;
use crate::values::{Circular, Knotted as _, Link, TypeValue, Value, cross};

use super::super::{KActivation, KValue, KValueFamily, Knotted, Untieable, tie};
use super::{Fixture, Step, copy, with_fixture};
use crate::scope::tests::plan;

/// Whether two words are the same binding: bit-identical scalars, one type, one knot node.
fn same(left: KValue<'_, '_>, right: KValue<'_, '_>) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.to_bits() == right.to_bits(),
        (Value::Null, Value::Null) => true,
        (Value::Type(left), Value::Type(right)) => left.handle() == right.handle(),
        (Value::Knotted(left), Value::Knotted(right)) => ptr::eq(left.node(), right.node()),
        (left, right) => {
            panic!(
                "a planned binding is a number, null, a type or a knot member: {left:?}, {right:?}"
            )
        }
    }
}

/// Whether `copied` is `original` rebuilt: the same scalars and types, and a knot member that is
/// the same node of a knot of the same size — a function of the same body.
fn rebuilt(original: KValue<'_, '_>, copied: KValue<'_, '_>) -> bool {
    match (original, copied) {
        (Value::Knotted(original), Value::Knotted(copied)) => {
            let bodies = match (original.function(), copied.function()) {
                (Some(original), Some(copied)) => ptr::eq(original.shape(), copied.shape()),
                (None, None) => true,
                _ => false,
            };
            bodies
                && original.member().index() == copied.member().index()
                && original.member().knot().len() == copied.member().knot().len()
        }
        (original, copied) => same(original, copied),
    }
}

/// The value a runner binds a slot the tie does not: a fresh number, or a type for a type name.
fn synthetic<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    writer: Writer<'cell>,
    name: BinderSymbol,
    next: &mut f64,
) -> KValue<'graph, 'cell> {
    match name {
        BinderSymbol::Type(_) => Value::Type(TypeValue::new(writer, KType::ANY, fixture.types)),
        BinderSymbol::Value(_) => {
            *next += 1.0;
            Value::Number(*next)
        }
    }
}

/// The planned reads of the data member at `slot`, one per item of its list literal: the component
/// index of a fellow member it reads, the coordinate of any other read, or `None` for a
/// parenthesized item the caller evaluates.
fn planned_items(
    shape: &Shape<'_>,
    component: &Component<'_>,
    slot: Slot,
) -> Vec<Option<Result<u32, Coordinate>>> {
    let Some(ExpressionPart::ListLiteral(items)) = shape.rhs(slot) else {
        panic!("a data binder of a tied component renders as a list literal");
    };
    items
        .iter()
        .map(|item| match item {
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => {
                let mention = shape
                    .mention(Site::of(item))
                    .expect("a name item is a read");
                Some(match mention.coordinate {
                    Coordinate::Activation {
                        hops: 0,
                        target: Target::Local(bound),
                    } if component.members.contains(&bound) => {
                        Ok(component.members.binary_search(&bound).unwrap() as u32)
                    }
                    coordinate => Err(coordinate),
                })
            }
            ExpressionPart::Expression(_) => None,
            other => panic!("a planned list item is a name or a parenthesized node: {other:?}"),
        })
        .collect()
}

/// Every link `member` holds: a function's closure bindings, or a planned data node's cells.
fn links<'graph, 'cell>(
    member: Knotted<'graph, 'cell>,
) -> Vec<Link<'cell, 'cell, Knotted<'graph, 'cell>>> {
    match member.function() {
        Some(function) => (0..function.closure().len())
            .map(|at| function.closure().get(CaptureSlot(at as u32)))
            .collect(),
        None => match Value::Knotted(member).as_circular() {
            Some((_, Circular::List(list))) => list.cells().to_vec(),
            _ => panic!("a planned data node is a list"),
        },
    }
}

/// Whether the reads among `data` members, `reads[i]` the members member `i` reads, hold a cycle.
fn cyclic(data: &[usize], reads: &[Vec<usize>]) -> bool {
    let mut left: Vec<usize> = data.to_vec();
    loop {
        let before = left.len();
        let remaining = left.clone();
        left.retain(|member| reads[*member].iter().any(|read| remaining.contains(read)));
        if left.is_empty() {
            return false;
        }
        if left.len() == before {
            return true;
        }
    }
}

/// Bring every binding of `activation` into being, component by component, checking each tie
/// against its members' capture layouts and planned reads; then activate every nested shape a call
/// or an arm would, and do the same there. Every tied member is pushed onto `tied`.
fn run<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    writer: Writer<'cell>,
    activation: &'cell KActivation<'graph, 'cell>,
    next: &mut f64,
    tied: &mut Vec<Knotted<'graph, 'cell>>,
) {
    let shape = activation.shape();
    let mut born: Vec<(Slot, Knotted<'graph, 'cell>)> = Vec::new();
    for component in shape.components() {
        let values = component
            .members
            .iter()
            .all(|slot| matches!(shape.slot_name(*slot), BinderSymbol::Value(_)));
        let births = component
            .members
            .iter()
            .filter(|slot| shape.births(**slot).is_some())
            .count();
        let synthesize = |next: &mut f64| {
            for slot in component.members {
                let value = synthetic(fixture, writer, shape.slot_name(*slot), next);
                activation.bind(*slot, value).expect("a fresh slot binds");
            }
        };
        if !values || !(component.cyclic || births == component.members.len()) {
            synthesize(next);
            continue;
        }
        let outcome = tie(
            writer,
            activation,
            component,
            fixture.types,
            fixture.scratch(),
            &mut |_| Some(Value::Null),
        );
        let data: Vec<usize> = (0..component.members.len())
            .filter(|index| shape.births(component.members[*index]).is_none())
            .collect();
        let reads: Vec<Vec<usize>> = component
            .members
            .iter()
            .map(|slot| match shape.births(*slot) {
                Some(_) => Vec::new(),
                None => planned_items(shape, component, *slot)
                    .into_iter()
                    .filter_map(|item| match item {
                        Some(Ok(index)) => Some(index as usize),
                        _ => None,
                    })
                    .collect(),
            })
            .collect();
        if cyclic(&data, &reads) {
            let Err(Untieable::TypeCycle { names }) = outcome else {
                panic!(
                    "a cycle of data members refuses the tie, got {:?}",
                    outcome.map(|_| ())
                );
            };
            assert!(!names.is_empty());
            for name in names {
                assert!(
                    data.iter()
                        .any(|index| shape.slot_name(component.members[*index]) == *name),
                    "a type cycle names data members"
                );
            }
            synthesize(next);
            continue;
        }
        let knot = outcome.expect("a component whose data members read acyclically ties");
        assert_eq!(knot.len() as usize, component.members.len());
        for (index, slot) in component.members.iter().enumerate() {
            let member = Knotted::of(knot, index);
            born.push((*slot, member));
            tied.push(member);
            let Some(body) = shape.births(*slot) else {
                let (_, Circular::List(list)) = Value::Knotted(member)
                    .as_circular()
                    .expect("a data member is a data node")
                else {
                    panic!("a planned data member is a list node");
                };
                let items = planned_items(shape, component, *slot);
                assert_eq!(list.len(), items.len());
                for (cell, item) in list.cells().iter().zip(items) {
                    match (item, cell) {
                        (Some(Ok(index)), Link::Edge(edge)) => assert_eq!(edge.index(), index),
                        (Some(Err(coordinate)), Link::Value(value)) => {
                            let Binding::Bound(enclosing) = activation.read(coordinate) else {
                                panic!("every enclosing binding is bound before the tie");
                            };
                            assert!(same(*value, enclosing), "a read cell is the enclosing word");
                        }
                        (None, Link::Value(Value::Null)) => {}
                        (item, _) => panic!("a data cell does not follow its read {item:?}"),
                    }
                }
                continue;
            };
            let function = member.function().expect("a birth is a function node");
            assert!(ptr::eq(function.shape(), body));
            assert_eq!(function.closure().len(), body.captures().len());
            for (at, spec) in body.captures().iter().enumerate() {
                match (spec.source, function.closure().get(CaptureSlot(at as u32))) {
                    (CaptureSource::Member { index, .. }, Link::Edge(edge)) => {
                        assert_eq!(edge.index(), index)
                    }
                    (CaptureSource::Read(coordinate), Link::Value(value)) => {
                        let Binding::Bound(enclosing) = activation.read(coordinate) else {
                            panic!("every enclosing binding is bound before the tie");
                        };
                        assert!(same(value, enclosing), "a capture is the enclosing word");
                    }
                    (source, _) => {
                        panic!("a closure binding does not follow its source {source:?}")
                    }
                }
            }
        }
        for (slot, member) in &born[born.len() - component.members.len()..] {
            activation
                .bind(*slot, Value::Knotted(*member))
                .expect("a fresh slot binds");
        }
    }

    for (_, nested) in shape.nested_shapes() {
        let nested_activation = match nested.kind() {
            ShapeKind::Block => KActivation::of_block(writer, nested, activation),
            ShapeKind::Callable => {
                let Some((_, callable)) = born.iter().find(|(_, member)| {
                    member
                        .function()
                        .is_some_and(|function| ptr::eq(function.shape(), *nested))
                }) else {
                    // A function value that is no binder's right-hand side: born by no tie.
                    continue;
                };
                let function = callable.function().expect("a function");
                KActivation::of_callable(
                    writer,
                    nested,
                    *callable,
                    function.closure(),
                    activation.builtins(),
                )
            }
            ShapeKind::Program | ShapeKind::Module => panic!("a plan nests no such shape"),
        };
        run(
            fixture,
            writer,
            resident(writer, nested_activation),
            next,
            tied,
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: crate::tests::case_share(1, 1), ..ProptestConfig::default() })]

    /// Every component of a planned program — and of every body and arm inside it — that is cyclic
    /// or births only callables ties when its data members read one another acyclically, each
    /// closure following its body's capture layout and each data node its planned reads, and
    /// refuses with a type cycle of data members otherwise; every tied knot crosses under a copy as
    /// the same knot rebuilt, each edge verbatim and each held value the original's.
    #[test]
    fn a_planned_component_ties_to_its_capture_layout_and_copies_whole(choices in plan::choices()) {
        let source = plan::program_source(&choices);
        with_fixture(|fixture| {
            let lines = fixture.parse(&source);
            let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
            let home = graph.create(None, None).unwrap();
            let dest = graph.create(None, None).unwrap();
            graph
                .enter(home, |context| {
                    let writer = context.writer();
                    let builtins = fixture.builtins(writer, &[]);
                    let shape = crate::scope::Shape::of_program(fixture.program, &lines, builtins, fixture.scratch())
                        .unwrap_or_else(|error| panic!("`{source}` shapes: {}", error.display(fixture.labels)));
                    let activation = resident(writer, KActivation::of_program(writer, shape, builtins));
                    let mut tied = Vec::new();
                    run(fixture, writer, activation, &mut 0.0, &mut tied);

                    for original in tied {
                        let carrier = context.lift::<KValueFamily>(Value::Knotted(original));
                        let crossed = cross(context, dest, &carrier).unwrap();
                        let Value::Knotted(copied) = context.read(&crossed).value() else {
                            panic!("a knot member crosses as a knot member");
                        };
                        assert!(!ptr::eq(copied.node(), original.node()), "`{source}`");
                        assert!(rebuilt(Value::Knotted(original), Value::Knotted(copied)));
                        assert_eq!(copied.weight(), original.weight());
                        for node in 0..original.member().knot().len() {
                            let edge = |callable: Knotted<'_, '_>| {
                                callable.member().knot().members().nth(node as usize).unwrap().index()
                            };
                            let (before, after): (Edge, Edge) = (edge(original), edge(copied));
                            let (before, after) = (original.sibling(before), copied.sibling(after));
                            assert_eq!(before.ktype(), after.ktype());
                            let (before, after) = (links(before), links(after));
                            assert_eq!(before.len(), after.len());
                            for (before, after) in before.into_iter().zip(after) {
                                match (before, after) {
                                    (Link::Edge(before), Link::Edge(after)) => assert_eq!(before, after),
                                    (Link::Value(before), Link::Value(after)) => {
                                        assert!(rebuilt(before, after), "`{source}`")
                                    }
                                    _ => panic!("a copy keeps each link's kind"),
                                }
                            }
                        }
                    }
                })
                .unwrap();
            graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
            graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        });
    }
}
