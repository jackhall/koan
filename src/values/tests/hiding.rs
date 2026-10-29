//! The `hides` bit a container or tagged value carries, and the copy that clears it: set wherever
//! a value or a part below it may store a field the type it is held at hides.

use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;

use super::{List, Record, Value, copy, pin, with_fixture};
use crate::memory::CellGraph;
use crate::values::ValueFamily;
use crate::values::hiding::narrowed;

use super::TestGraph;

/// The hides bit of a container or tagged value.
fn hides(value: &Value<'_>) -> bool {
    match value {
        Value::List(list) => list.weight().hides(),
        Value::Dict(dict) => dict.weight().hides(),
        Value::Record(record) => record.weight().hides(),
        Value::Tagged(tagged) => tagged.weight().hides(),
        _ => false,
    }
}

#[test]
fn a_restamp_hiding_a_stored_field_sets_the_bit_and_a_copy_clears_it() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
            let name = |text| BinderSymbol::declared(text, symbols).unwrap();
            let (x, y) = (name("x"), name("y"));
            let n = KType::NUMBER;
            let fields = [(x, Value::Number(1.0)), (y, Value::Number(2.0))];
            let record = Value::Record(Record::new(writer, &fields, types, scratch));
            assert!(
                !hides(&record),
                "a record literal stores what its type names"
            );
            let list = Value::List(List::new(writer, [record].into_iter(), types, scratch));
            assert!(!hides(&list), "a list of it hides nothing");
            assert!(!hides(&record.retyped(
                writer,
                record.concrete_ktype(),
                types,
                scratch
            )));
            let narrow = types.record(scratch, &[(x, n)]);
            let hidden = record.retyped(writer, narrow, types, scratch);
            assert!(hides(&hidden), "a retype hiding `y` sets the bit");
            // A field typed exactly `Null` holds no value to hide.
            let null = [(x, Value::Number(1.0)), (y, Value::Null)];
            let null = Value::Record(Record::new(writer, &null, types, scratch));
            assert!(!hides(&null.retyped(writer, narrow, types, scratch)));
            // Held at a type naming `y` at `Null`, the hiding record is copied without it.
            let filled = types.record(scratch, &[(x, n), (y, KType::NULL)]);
            let wide = types.record(scratch, &[(x, n), (y, n)]);
            let element = types.union_of(scratch, &[filled, wide]);
            let other = Value::Record(Record::new(writer, &fields, types, scratch));
            let both = List::new(writer, [hidden, other].into_iter(), types, scratch);
            assert_eq!(both.ktype(), types.list(element));
            let Value::Record(copied) =
                crate::values::tests::parts(Value::List(both), types, scratch)[0]
            else {
                panic!("a record element");
            };
            assert_eq!(copied.names().len(), 1, "the copy stores `x` alone");
            assert!(!hides(&Value::List(both)), "the copy hides nothing");
        })
    });
}

#[test]
fn a_crossing_copy_clears_the_bit() {
    with_fixture(|fixture| {
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
                let name = |text| BinderSymbol::declared(text, symbols).unwrap();
                let (x, y) = (name("x"), name("y"));
                let fields = [(x, Value::Number(1.0)), (y, Value::Number(2.0))];
                let record = Value::Record(Record::new(writer, &fields, types, scratch));
                let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
                let hidden = record.retyped(writer, narrow, types, scratch);
                assert!(hides(&hidden));
                let source = context.lift::<ValueFamily>(hidden);
                let crossed = fixture.tiers.cross(context, dest, &source).unwrap();
                assert!(!hides(&context.read(&crossed).value()));
            })
            .unwrap();
    });
}

#[test]
fn a_record_rebuilt_over_a_copy_keeps_the_bit_for_a_field_its_type_lacks() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
            let name = |text| BinderSymbol::declared(text, symbols).unwrap();
            let (a, g, x, y) = (name("a"), name("g"), name("x"), name("y"));
            let inner = [(x, Value::Number(1.0)), (g, Value::Number(5.0))];
            let inner = Value::Record(Record::new(writer, &inner, types, scratch));
            let outer = [(a, inner), (y, Value::Number(3.0))];
            let outer = Value::Record(Record::new(writer, &outer, types, scratch));
            let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
            let narrow = types.record(scratch, &[(a, narrow)]);
            let hidden = outer.retyped(writer, narrow, types, scratch);
            // Beside it, `a` is seen naming `g` at `Null`: its `g` is dropped, and `outer` rebuilt.
            let other = [(x, Value::Number(2.0)), (g, Value::Null)];
            let other = Value::Record(Record::new(writer, &other, types, scratch));
            let other = Value::Record(Record::new(writer, &[(a, other)], types, scratch));
            let both = List::new(writer, [hidden, other].into_iter(), types, scratch);
            let rebuilt = crate::values::tests::parts(Value::List(both), types, scratch)[0];
            let Value::Record(kept) = rebuilt else {
                panic!("a record element");
            };
            assert_eq!(kept.names().len(), 2, "the rebuilt record still stores `y`");
            let at = kept.names().iter().position(|name| *name == a.symbol());
            let Value::Record(copied) = kept.cells()[at.expect("`a` is stored")] else {
                panic!("a record field");
            };
            assert_eq!(copied.names().len(), 1, "`a` is copied without `g`");
            assert!(hides(&rebuilt), "its type does not name `y`");
        })
    });
}

#[test]
fn a_part_narrowed_by_its_view_keeps_its_own_type_less_what_the_view_hides() {
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let name = |text| BinderSymbol::declared(text, symbols).unwrap();
        let (a, g, n, x, y, z) = (
            name("a"),
            name("g"),
            name("n"),
            name("x"),
            name("y"),
            name("z"),
        );
        let (number, null) = (KType::NUMBER, KType::NULL);
        let record = |fields: &[(BinderSymbol, KType)]| types.record(scratch, fields);
        let narrowed = |own, view| narrowed(types, scratch, own, view);
        let (just_x, xy) = (record(&[(x, number)]), record(&[(x, number), (y, number)]));
        assert_eq!(narrowed(xy, just_x), just_x, "a hidden field is dropped");
        let filled = record(&[(g, null), (x, number)]);
        assert_eq!(
            narrowed(just_x, filled),
            just_x,
            "a fill's `Null` is not added"
        );
        assert_eq!(narrowed(xy, KType::ANY), xy, "`Any` hides nothing");
        assert_eq!(
            narrowed(just_x, record(&[(x, null)])),
            record(&[]),
            "a field seen at exactly `Null` over a value is dropped"
        );
        let kept = record(&[(n, null), (x, number), (y, number)]);
        assert_eq!(
            narrowed(kept, just_x),
            record(&[(n, null), (x, number)]),
            "a field its own type has at `Null` holds no value, and stays"
        );
        assert_eq!(
            narrowed(record(&[(a, xy)]), record(&[(a, just_x)])),
            record(&[(a, just_x)]),
            "a field narrows by the view's field"
        );
        assert_eq!(
            narrowed(types.list(xy), types.list(filled)),
            types.list(just_x),
            "an element narrows by the view's element"
        );
        let first = record(&[(x, number), (y, number), (z, null)]);
        let second = record(&[(x, number), (y, null), (z, number)]);
        let own = types.union_of(scratch, &[first, second]);
        let each = [
            record(&[(x, number), (z, null)]),
            record(&[(x, number), (y, null)]),
        ];
        assert_eq!(
            narrowed(types.list(own), types.list(just_x)),
            types.list(types.union_of(scratch, &each)),
            "each member narrows by what the door takes it at"
        );
    });
}

#[test]
fn a_crossing_copy_keeps_each_parts_own_type_where_its_holder_fills_it() {
    with_fixture(|fixture| {
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
                let name = |text| BinderSymbol::declared(text, symbols).unwrap();
                let (g, x) = (name("g"), name("x"));
                // `[[{x = 1}], [{g = 4, x = 9}]]`: the fill sees the first record at `g :Null`.
                let one = [(x, Value::Number(1.0))];
                let one = Value::Record(Record::new(writer, &one, types, scratch));
                let first = Value::List(List::new(writer, [one].into_iter(), types, scratch));
                let nine = [(g, Value::Number(4.0)), (x, Value::Number(9.0))];
                let nine = Value::Record(Record::new(writer, &nine, types, scratch));
                let second = Value::List(List::new(writer, [nine].into_iter(), types, scratch));
                let own = first.concrete_ktype();
                let both = List::new(writer, [first, second].into_iter(), types, scratch);
                let both = Value::List(both);
                let source = context.lift::<ValueFamily>(both);
                let crossed = fixture.tiers.cross(context, dest, &source).unwrap();
                let copied = context.read(&crossed).value().as_list().expect("a list");
                let inner = copied.cells()[0];
                assert_eq!(
                    inner.concrete_ktype(),
                    own,
                    "the inner list keeps its own type"
                );
                let element = inner.as_list().expect("a list").cells()[0];
                assert_eq!(
                    element.concrete_ktype(),
                    types.record(scratch, &[(x, KType::NUMBER)]),
                    "the record keeps its own type"
                );
            })
            .unwrap();
    });
}
