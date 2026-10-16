//! A knot's memo bits: kept on its root node and read through every member.

use crate::memory::{CellGraph, ReleaseAbsorption};
use crate::values::{Knotted as _, MemoBits, Value, cross};

use super::super::{KValueFamily, Knotted};
use super::{TestGraph, bound, callable, copy, pin, with_fixture};

#[test]
fn a_knots_bits_sit_on_its_root_node() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET f = (FN :{x :Number} -> Number = #(g {x = x}))
LET g = (FN :{x :Number} -> Number = #(f {x = x}))",
        );
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let activation = fixture.run(context.writer(), types, &lines, &[]);
            let knot = callable(fixture, activation, "f").member().knot();
            assert_eq!(knot.len(), 2);
            let (root, other) = (Knotted::of(knot, 0), Knotted::of(knot, 1));
            let bytes = root.weight();
            assert_eq!(root.bits(), MemoBits::NONE);
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
            let (activation, types, _) =
                fixture.run_frozen(context.writer(), context.header(), &lines);
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
    // The fixture's own chain reads the builder as a region tier, so a birth through it settles
    // nothing.
    with_fixture(|fixture| {
        let lines = fixture.parse(CAPTURING);
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let activation = fixture.run(context.writer(), types, &lines, &[]);
            assert!(!callable(fixture, activation, "f").bits().settled);
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
                let (activation, _, root) =
                    fixture.run_frozen(context.writer(), context.header(), &lines);
                let f = callable(fixture, activation, "f");
                let born = f.bits();
                assert!(born.settled);
                // Cleared, so a copy that carried the bits over would arrive with none.
                f.set_bits(MemoBits::NONE);
                let source = context.lift::<KValueFamily>(Value::Knotted(f));
                let crossed = cross(context, dest, &source, root).unwrap();
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
