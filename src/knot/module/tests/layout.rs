//! A member step of a kept path: the slot a module stores a member at ([`layout::member_slot`]),
//! and the member that slot reaches ([`layout::stored_member`]).

use crate::knot::tests::{pin, with_fixture};
use crate::values::Value;

use super::super::layout;
use super::module;

#[test]
fn a_member_step_reads_back_the_member_a_named_read_restamps() {
    with_fixture(|fixture| {
        let lines =
            fixture.parse("MODULE m = ((LET zero = 0) (LET name = \"m\") (NEWTYPE Dist = Number))");
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let node = module(fixture, activation, "m");
            let m = Value::Knotted(node);
            let read = layout::member(writer, m, (fixture.name("name"), None), types, scratch);
            let read = read.expect("a value member");
            let slot = layout::member_slot(m, read).expect("the module stores the member read");
            let stored = layout::stored_member(m, slot).expect("a stored member");
            assert!(
                read.same_storage(&stored),
                "the member a named read restamps"
            );
            let past = node.module().expect("a module").members().len();
            assert!(layout::stored_member(m, past).is_none(), "past the run");
            assert!(layout::member_slot(m, Value::Number(1.0)).is_none());
            assert!(layout::stored_member(Value::Number(0.0), 0).is_none());
        });
    });
}
