//! The law, over `scope`'s generated shape plans: every component of callable binders ties to one
//! knot whose closures follow its members' capture layouts — an edge for a fellow member, the
//! enclosing word otherwise — a component holding a data binder refuses the tie, and a copy of every
//! knot is the same knot rebuilt.

use std::ptr;

use proptest::prelude::*;

use crate::memory::{CellGraph, Edge, ReleaseAbsorption, Writer, resident};
use crate::parse::BinderSymbol;
use crate::scope::{Binding, CaptureSlot, CaptureSource, ShapeKind, Slot};
use crate::type_lattice::KType;
use crate::values::Link;
use crate::values::{Knotted as _, TypeValue, Value, cross};

use super::super::{KActivation, KValue, KValueFamily, Knotted, Untieable, tie};
use super::{Fixture, Step, copy, with_fixture};
use crate::scope::tests::plan;

/// Whether two words are the same binding: bit-identical scalars, one type, one knot node.
fn same(left: KValue<'_, '_>, right: KValue<'_, '_>) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.to_bits() == right.to_bits(),
        (Value::Type(left), Value::Type(right)) => left.handle() == right.handle(),
        (Value::Knotted(left), Value::Knotted(right)) => ptr::eq(left.node(), right.node()),
        (left, right) => {
            panic!("a planned binding is a number, a type or a callable: {left:?}, {right:?}")
        }
    }
}

/// Whether `copied` is `original` rebuilt: the same scalars and types, and a callable that is the
/// same node of a knot of the same body.
fn rebuilt(original: KValue<'_, '_>, copied: KValue<'_, '_>) -> bool {
    match (original, copied) {
        (Value::Knotted(original), Value::Knotted(copied)) => {
            ptr::eq(
                original.function().expect("a function").shape(),
                copied.function().expect("a function").shape(),
            ) && original.member().index() == copied.member().index()
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

/// Bring every binding of `activation` into being, component by component, checking each tie
/// against its members' capture layouts; then activate every nested shape a call or an arm would,
/// and do the same there. Every tied callable is pushed onto `tied`.
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
        let births = component
            .members
            .iter()
            .filter(|slot| shape.births(**slot).is_some())
            .count();
        let outcome = tie(
            writer,
            activation,
            component,
            fixture.types,
            fixture.scratch(),
        );
        if births < component.members.len() {
            assert!(
                matches!(outcome, Err(Untieable::Data { .. })),
                "a component holding a data binder refuses the tie, got {:?}",
                outcome.err()
            );
            for slot in component.members {
                let value = synthetic(fixture, writer, shape.slot_name(*slot), next);
                activation.bind(*slot, value).expect("a fresh slot binds");
            }
            continue;
        }
        let knot = outcome.expect("a component of callable binders ties");
        assert_eq!(knot.len() as usize, component.members.len());
        for (index, slot) in component.members.iter().enumerate() {
            let callable = Knotted::of(knot, index);
            let body = shape.births(*slot).expect("every member births");
            let function = callable.function().expect("a function");
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
            born.push((*slot, callable));
            tied.push(callable);
        }
        for (slot, callable) in &born[born.len() - component.members.len()..] {
            activation
                .bind(*slot, Value::Knotted(*callable))
                .expect("a fresh slot binds");
        }
    }

    for (_, nested) in shape.nested_shapes() {
        let nested_activation = match nested.kind() {
            ShapeKind::Block => KActivation::of_block(writer, nested, activation),
            ShapeKind::Callable => {
                let Some((_, callable)) = born.iter().find(|(_, callable)| {
                    ptr::eq(callable.function().expect("a function").shape(), *nested)
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

    /// Every component of a planned program — and of every body and arm inside it — ties when its
    /// members all birth callables, with each closure following its body's capture layout, and
    /// refuses with a data member otherwise; every tied knot crosses under a copy as the same knot
    /// rebuilt, each edge verbatim and each captured value the original's.
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
                    let builtins = fixture.builtins(writer);
                    let shape = crate::scope::Shape::of_program(fixture.program, &lines, builtins, fixture.scratch())
                        .unwrap_or_else(|error| panic!("`{source}` shapes: {}", error.display(fixture.labels)));
                    let activation = resident(writer, KActivation::of_program(writer, shape, builtins));
                    let mut tied = Vec::new();
                    run(fixture, writer, activation, &mut 0.0, &mut tied);

                    for original in tied {
                        let carrier = context.lift::<KValueFamily>(Value::Knotted(original));
                        let crossed = cross(context, dest, &carrier).unwrap();
                        let Value::Knotted(copied) = context.read(&crossed).value() else {
                            panic!("a callable crosses as a callable");
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
                            let (before, after) = (before.function().expect("a function").closure(), after.function().expect("a function").closure());
                            assert_eq!(before.len(), after.len());
                            for at in 0..before.len() {
                                match (before.get(CaptureSlot(at as u32)), after.get(CaptureSlot(at as u32))) {
                                    (Link::Edge(before), Link::Edge(after)) => assert_eq!(before, after),
                                    (Link::Value(before), Link::Value(after)) => {
                                        assert!(rebuilt(before, after), "`{source}`")
                                    }
                                    _ => panic!("a copy keeps each binding's kind"),
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
