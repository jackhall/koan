//! A knot's memo bits: kept on its root node and read through every member.

use std::cell::Cell;

use crate::memory::{Bump, CellGraph, ReleaseAbsorption};
use crate::type_lattice::KType;
use crate::values::{
    ADOPTED, Destination, Knotted as _, MemoBits, PARTS_ENTERED, Retyped, Value, adopt_value, cross,
};

use super::super::{KValueFamily, Knotted, KnottedFamily};
use super::{TestGraph, bound, callable, copy, pin, with_fixture};

#[test]
fn a_knots_bits_sit_on_its_root_node() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET f = (FN :{x :Number} -> Number = #(g {x = x}))
LET g = (FN :{x :Number} -> Number = #(f {x = x}))",
        );
        fixture.in_cell(pin, |context| {
            // Born of the load's types, every memo it reaches is the root's; nothing has settled it.
            let types = &fixture.tiers.paired(context);
            let activation = fixture.run(context.writer(), types, &lines, &[]);
            let knot = callable(fixture, activation, "f").member().knot();
            assert_eq!(knot.len(), 2);
            let (root, other) = (Knotted::of(knot, 0), Knotted::of(knot, 1));
            let bytes = root.weight();
            let born = MemoBits {
                root_closed: true,
                settled: false,
            };
            assert_eq!(root.bits(), born);
            other.set_bits(MemoBits::BOTH);
            for member in [root, other] {
                assert_eq!(member.bits(), MemoBits::BOTH);
                assert_eq!(member.weight(), bytes);
            }
            let other_weight = other.function().expect("a function").knot_weight();
            assert_eq!(
                other_weight.bits(),
                MemoBits::NONE,
                "a non-root node keeps no bits"
            );
            assert_eq!(
                root.function().expect("a function").knot_weight().bits(),
                MemoBits::BOTH
            );
        });
    });
}

const CAPTURING: &str = "\
LET xs = [1 2]
LET f = (FN :{x :Number} -> Any = #(xs))
LET g = (FN :{x :Number} -> Number = #(x))";

#[test]
fn a_knot_born_through_its_regions_chain_is_settled_on_its_root() {
    with_fixture(|fixture| {
        let lines = fixture.parse(CAPTURING);
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let activation = fixture.run(context.writer(), types, &lines, &[]);
            let xs = bound(fixture, activation, "xs").as_list().expect("a list");
            assert!(
                xs.bits().settled,
                "the capture is laid through the same chain"
            );
            let f = callable(fixture, activation, "f");
            assert_eq!(
                f.bits(),
                MemoBits {
                    root_closed: xs.bits().root_closed && types.in_root(f.ktype()),
                    settled: true,
                }
            );
            // `g` captures nothing, so its own type alone decides root-closed.
            let g = callable(fixture, activation, "g");
            assert_eq!(
                g.bits(),
                MemoBits {
                    root_closed: types.in_root(g.ktype()),
                    settled: true,
                }
            );
        });
    });
    // A paired chain does not intern where it lays, so a birth through it settles nothing.
    with_fixture(|fixture| {
        let lines = fixture.parse(CAPTURING);
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.paired(context);
            let activation = fixture.run(context.writer(), types, &lines, &[]);
            assert!(!callable(fixture, activation, "f").bits().settled);
        });
    });
}

#[test]
fn an_exit_hands_over_a_headers_memo_and_stops_at_its_settled_member() {
    with_fixture(|fixture| {
        let lines = fixture.parse(CAPTURING);
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let f = callable(fixture, activation, "f");
            assert!(f.bits().settled);
            // What an exit into the region adopts for `value`, and how many parts it entered.
            let exited = |value: Value<'_, _>| {
                ADOPTED.with(|adopted| adopted.set(0));
                PARTS_ENTERED.with(|entered| entered.set(0));
                let into = Destination::Region(types);
                adopt_value::<KnottedFamily>(into, types, &value, &Bump::new());
                (ADOPTED.with(Cell::get), PARTS_ENTERED.with(Cell::get))
            };
            let header = Retyped::of(writer, f, KType::ANY.into());
            assert_eq!(
                exited(Value::Retyped(header)),
                (1, 0),
                "a header's own memo is handed over, its settled member is not entered"
            );
            assert_eq!(exited(Value::Knotted(f)), (0, 0));
        });
    });
}

#[test]
fn a_knot_copy_is_stamped_at_its_destination() {
    with_fixture(|fixture| {
        let lines = fixture.parse(CAPTURING);
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let activation = fixture.run(context.writer(), types, &lines, &[]);
                let f = callable(fixture, activation, "f");
                let born = f.bits();
                assert!(born.settled);
                // Cleared, so a copy that carried the bits over would arrive with none.
                f.set_bits(MemoBits::NONE);
                let source = context.lift::<KValueFamily>(Value::Knotted(f));
                let crossed = cross(context, dest, &source, fixture.tiers.tiers()).unwrap();
                let Value::Knotted(copied) = context.read(&crossed).value() else {
                    panic!("a callable crosses as a callable");
                };
                assert_eq!(copied.bits(), born);
                assert_eq!(f.bits(), MemoBits::NONE, "the source is unchanged");
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}
