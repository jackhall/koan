//! A knot's memo bits: kept on its root node and read through every member.

use crate::values::{Knotted as _, MemoBits};

use super::super::Knotted;
use super::{callable, pin, with_fixture};

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
