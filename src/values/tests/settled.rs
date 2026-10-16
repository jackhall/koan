//! The memo bits a weight carries — masked out of its bytes and its equality, dropped by a sum —
//! and how a value door, a restamp and a copy set them.

use crate::memory::{CellGraph, ReleaseAbsorption};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{MemoBits, ValueFamily, Weight, cross};

use super::{Fixture, List, Record, TestGraph, Value, copy, pin, with_fixture};

#[test]
fn a_weights_bits_are_masked_out_of_its_bytes_and_its_equality() {
    let plain = Weight::text(40);
    let marked = plain.with_bits(MemoBits::BOTH);
    assert_eq!(marked.bytes(), 40);
    assert_eq!(marked.bits(), MemoBits::BOTH);
    assert_eq!(marked, plain, "equality reads the bytes alone");
    assert_eq!(marked.cmp(&plain), std::cmp::Ordering::Equal);
    assert_eq!(marked.without_bits().bits(), MemoBits::NONE);
    let settled = MemoBits {
        root_closed: false,
        settled: true,
    };
    assert_eq!(plain.with_bits(settled).bits(), settled);
    assert!(settled.meets() && !MemoBits::NONE.meets());
}

#[test]
fn a_sum_of_weights_carries_no_bits_and_saturates_below_them() {
    let marked = Weight::text(8).with_bits(MemoBits::BOTH);
    let sum = marked.plus(marked);
    assert_eq!(sum.bytes(), 16);
    assert_eq!(sum.bits(), MemoBits::NONE);
    let full = Weight::text(usize::MAX);
    assert_eq!(
        full.bits(),
        MemoBits::NONE,
        "a saturated count reaches no bit"
    );
    assert_eq!(full.plus(full).bits(), MemoBits::NONE);
    assert_eq!(full.plus(Weight::text(1)), full);
    assert_eq!(Weight::run::<u64>(usize::MAX).bits(), MemoBits::NONE);
}

/// Two field names a test's records are built over.
fn names(fixture: &Fixture<'_, '_>) -> (BinderSymbol, BinderSymbol) {
    let name = |text| BinderSymbol::declared(text, fixture.symbols).expect("a value name");
    (name("outer"), name("inner"))
}

#[test]
fn a_composite_over_root_types_is_root_closed_whatever_chain_built_it() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let numbers = || [Value::Number(1.0), Value::Number(2.0)].into_iter();
            let own = &fixture.tiers.own(context);
            let shares = &fixture.tiers.shares(context);
            let laid = List::typed(writer, numbers(), KType::LIST_OF_ANY, own);
            assert_eq!(laid.bits(), MemoBits::BOTH);
            let porous = List::typed(writer, numbers(), KType::LIST_OF_ANY, shares);
            assert_eq!(
                porous.bits(),
                MemoBits {
                    root_closed: true,
                    settled: false,
                }
            );
        })
    });
}

#[test]
fn a_composite_is_settled_only_through_a_chain_that_interns_where_it_lays() {
    with_fixture(|fixture| {
        let (outer, inner) = names(fixture);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let own = &fixture.tiers.own(context);
            let shares = &fixture.tiers.shares(context);
            let settled = MemoBits {
                root_closed: false,
                settled: true,
            };
            // `{inner :Number}` is no root type, so it lands in the cell's branch.
            let part = Record::new(writer, &[(inner, Value::Number(1.0))], own, scratch);
            assert_eq!(part.bits(), settled);
            let fields = [(outer, Value::Record(part))];
            assert_eq!(Record::new(writer, &fields, own, scratch).bits(), settled);
            let porous = Record::new(writer, &[(inner, Value::Number(1.0))], shares, scratch);
            assert_eq!(porous.bits(), MemoBits::NONE);
        })
    });
}

#[test]
fn a_part_meeting_no_contract_clears_its_holders_settled_bit() {
    with_fixture(|fixture| {
        let (_, inner) = names(fixture);
        let scratch = fixture.scratch();
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let own = &fixture.tiers.own(context);
            let shares = &fixture.tiers.shares(context);
            let porous = Record::new(writer, &[(inner, Value::Number(1.0))], shares, scratch);
            // Typed `List Any`, so the region chain never reads the part's scratch memo.
            let list = List::typed(
                writer,
                [Value::Record(porous)].into_iter(),
                KType::LIST_OF_ANY,
                own,
            );
            assert_eq!(list.bits(), MemoBits::NONE);
        })
    });
}

#[test]
fn a_restamp_recomputes_both_bits_from_its_new_type() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let own = &fixture.tiers.own(context);
            let shares = &fixture.tiers.shares(context);
            let numbers = [Value::Number(1.0)].into_iter();
            let list = List::typed(writer, numbers, KType::LIST_OF_ANY, own);
            assert_eq!(list.bits(), MemoBits::BOTH);
            // `List Number` is no root type: the region chain interns it in the cell's branch,
            // the porous one in the scratch's.
            let narrowed = list.with_type(writer, own.list(KType::NUMBER), own);
            assert_eq!(
                narrowed.bits(),
                MemoBits {
                    root_closed: false,
                    settled: true,
                }
            );
            let porous = list.with_type(writer, shares.list(KType::NUMBER), shares);
            assert_eq!(porous.bits(), MemoBits::NONE);
            assert_eq!(list.bits(), MemoBits::BOTH, "a restamp leaves the original");
        })
    });
}

#[test]
fn a_copy_is_settled_at_its_destination() {
    with_fixture(|fixture| {
        let (outer, inner) = names(fixture);
        let scratch = fixture.scratch();
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                // The fixture's chain reads the builder as a region tier, so it settles nothing.
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let part = Record::new(writer, &[(inner, Value::Number(1.0))], types, scratch);
                let fields = [(outer, Value::Record(part))];
                let record = Record::new(writer, &fields, types, scratch);
                assert_eq!(record.bits(), MemoBits::NONE);
                let source = context.lift::<ValueFamily>(Value::Record(record));
                let crossed = cross(context, dest, &source, fixture.tiers.root).unwrap();
                assert_eq!(record.bits(), MemoBits::NONE, "the original is unchanged");
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Record(copied) = context.read(&carrier).value() else {
                    panic!("a record crosses as a record");
                };
                let settled = MemoBits {
                    root_closed: false,
                    settled: true,
                };
                assert_eq!(copied.bits(), settled);
                let Some(Value::Record(part)) = copied.cells().first() else {
                    panic!("the copy holds its part");
                };
                assert_eq!(part.bits(), settled);
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}
