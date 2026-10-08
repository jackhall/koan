//! Content digests: equal content digests alike wherever it was laid down, a retype digests at its
//! new type and a hidden cell counts for nothing, so a copy digests as its source, and a part
//! shared many times over is digested once per demand.

use crate::memory::{CellGraph, ReleaseAbsorption};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{Key, ValueFamily, cross, record_type};

use super::{Dict, List, Record, Step, Value, copy, pin, text, with_fixture};

#[test]
fn equal_content_digests_alike_in_two_cells() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let build = || {
            fixture.in_cell(pin, |context| {
                let writer = context.writer();
                let cells = [Value::Number(1.0), text(writer, "a"), Value::Null];
                Value::List(List::new(writer, cells.into_iter(), types, scratch))
                    .digest(types, scratch)
            })
        };
        assert_eq!(build(), build());
    });
}

#[test]
fn a_dict_digests_in_key_order_and_a_record_blind_to_field_order() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let name = |text| BinderSymbol::declared(text, fixture.symbols).unwrap();
        let (x, y) = (name("x"), name("y"));
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (one, two) = (Value::Number(1.0), Value::Number(2.0));
            let (a, b) = (Key::str("a"), Key::str("b"));
            let ab = Dict::new(writer, &[(a, one), (b, two)], types, scratch);
            let ba = Dict::new(writer, &[(b, two), (a, one)], types, scratch);
            let digest = |value: Value<'_>| value.digest(types, scratch);
            assert_eq!(digest(Value::Dict(ab)), digest(Value::Dict(ba)));
            let swapped = Dict::new(writer, &[(a, two), (b, one)], types, scratch);
            assert_ne!(digest(Value::Dict(ab)), digest(Value::Dict(swapped)));

            let xy = Record::new(writer, &[(x, one), (y, two)], types, scratch);
            let yx = Record::new(writer, &[(y, two), (x, one)], types, scratch);
            assert_eq!(digest(Value::Record(xy)), digest(Value::Record(yx)));
        })
    });
}

/// A retype digests at its new type, which is part of the recipe, so restamping the type alone
/// changes the digest; a cell the new type hides is no part of what the door shows, so it counts
/// for nothing.
#[test]
fn a_retype_changes_the_digest_and_a_hidden_cell_does_not() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let name = |text| BinderSymbol::declared(text, fixture.symbols).unwrap();
        let (x, y) = (name("x"), name("y"));
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let digest = |value: Value<'_>| value.digest(types, scratch);
            let wide = Record::new(
                writer,
                &[(x, Value::Number(1.0)), (y, Value::Number(2.0))],
                types,
                scratch,
            );
            let only_x = record_type(types, scratch, [(x, KType::NUMBER)].into_iter());
            let narrowed = wide.with_type(writer, only_x);
            assert_ne!(digest(Value::Record(wide)), digest(Value::Record(narrowed)));
            let narrow = Record::new(writer, &[(x, Value::Number(1.0))], types, scratch);
            assert_eq!(narrow.ktype(), narrowed.ktype());
            assert_eq!(
                digest(Value::Record(narrow)),
                digest(Value::Record(narrowed)),
                "the hidden `y` is no part of what the retyped record shows"
            );

            let numbers = Value::List(List::new(
                writer,
                [Value::Number(1.0)].into_iter(),
                types,
                scratch,
            ));
            let restamped = numbers.retyped(writer, KType::LIST_OF_ANY, types, scratch);
            assert_ne!(
                digest(numbers),
                digest(restamped),
                "a retype that hides nothing still restamps the type"
            );
        })
    });
}

/// Each of the four ways a copy lays down less than, or other than, its source's cells: a retyped
/// record, a list whose join hides a record's field, a ring's knot copied whole, and a data node
/// seen narrower than its memo. The copy holds what the door showed, so it digests as its source.
#[test]
fn a_copy_digests_as_its_source() {
    use super::{Holding, Link, NodeFamily, ring, tie};
    use crate::values::Circular;
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let name = |text| BinderSymbol::declared(text, fixture.symbols).unwrap();
        let (x, y, next, value, held) = (
            name("x"),
            name("y"),
            name("next"),
            name("value"),
            name("held"),
        );
        let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let writer = context.writer();

                let fields = [(x, Value::Number(1.0)), (y, text(writer, "a"))];
                let record = Record::new(writer, &fields, types, scratch);
                let retyped = Value::Record(record).retyped(writer, narrow, types, scratch);
                let source = context.lift::<ValueFamily>(retyped);
                let crossed = cross(context, dest, &source, types).unwrap();
                let copied = context.read(&crossed).value();
                assert_eq!(
                    copied.digest(types, scratch),
                    retyped.digest(types, scratch),
                    "a retyped record"
                );

                let fields = [(x, Value::Number(1.0)), (y, Value::Number(2.0))];
                let wide = Value::Record(Record::new(writer, &fields, types, scratch));
                let only_x = Value::Record(Record::new(
                    writer,
                    &[(x, Value::Number(3.0))],
                    types,
                    scratch,
                ));
                let list = List::new(writer, [wide, only_x].into_iter(), types, scratch);
                assert_eq!(list.ktype(), types.list(narrow), "the join hides `y`");
                let joined = Value::List(list);
                let source = context.lift::<ValueFamily>(joined);
                let crossed = cross(context, dest, &source, types).unwrap();
                let copied = context.read(&crossed).value();
                assert_eq!(
                    copied.digest(types, scratch),
                    joined.digest(types, scratch),
                    "a list whose join hides a record's field"
                );

                let identity = fixture.ring_type("Ring", "next");
                let held_text = crate::values::text(writer, "held");
                let members = ring(fixture, writer, identity, &[Some(held_text), None]);
                let Some(Link::Edge(payload)) = Holding::Knotted(members[0])
                    .as_circular()
                    .and_then(|(_, node)| match node {
                        Circular::Tagged(tagged) => Some(*tagged.payload()),
                        _ => None,
                    })
                else {
                    panic!("a ring member is a tagged node over an edge");
                };
                let record_node = crate::values::Knotted::sibling(&members[0], payload);
                let both = [Holding::Knotted(members[0]), Holding::Knotted(record_node)];
                let pair = Holding::List(crate::values::List::new(
                    writer,
                    both.into_iter(),
                    types,
                    scratch,
                ));
                let source = context.lift::<ValueFamily<NodeFamily>>(pair);
                let crossed = cross(context, dest, &source, types).unwrap();
                let copied = context.read(&crossed).value();
                assert_eq!(
                    copied.digest(types, scratch),
                    pair.digest(types, scratch),
                    "a ring's knot copied whole"
                );

                let memo = types.record(scratch, &[(next, KType::ANY), (value, KType::NUMBER)]);
                let only_value = types.record(scratch, &[(value, KType::NUMBER)]);
                let node = tie(writer, 1, |_, edges| {
                    let fields = [
                        (next, Link::Edge(edges[0])),
                        (value, Link::Value(Holding::Number(1.0))),
                    ];
                    Circular::Record(crate::values::Record::linked(
                        writer, &fields, memo, scratch,
                    ))
                })[0];
                let holder = crate::values::Record::new(
                    writer,
                    &[(held, Holding::Knotted(node))],
                    types,
                    scratch,
                );
                let shown = types.record(scratch, &[(held, only_value)]);
                let narrowed = Holding::Record(holder).retyped(writer, shown, types, scratch);
                let source = context.lift::<ValueFamily<NodeFamily>>(narrowed);
                let crossed = cross(context, dest, &source, types).unwrap();
                let copied = context.read(&crossed).value();
                let digest = narrowed.digest(types, scratch);
                assert_eq!(
                    copied.digest(types, scratch),
                    digest,
                    "a data node seen narrower than its memo"
                );
                let plain = Record::new(writer, &[(value, Value::Number(1.0))], types, scratch);
                let fresh = Record::new(writer, &[(held, Value::Record(plain))], types, scratch);
                assert_eq!(
                    Value::Record(fresh).digest(types, scratch),
                    digest,
                    "the node digests as the plain value the door shows"
                );
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

/// Each level holds the one below twice over, so a walk that digested a part each time it met one
/// would hash `2^64` lists; one memo per demand hashes each once.
#[test]
fn a_part_shared_many_times_over_is_digested_once_per_demand() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let mut level = Value::Number(1.0);
            for _ in 0..64 {
                level = Value::List(List::new(
                    writer,
                    [level, level].into_iter(),
                    types,
                    scratch,
                ));
            }
            assert_eq!(level.digest(types, scratch), level.digest(types, scratch));
        })
    });
}
