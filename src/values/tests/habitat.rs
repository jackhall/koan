//! Where a crossed value's types land: a pinned value's home branch merges into its destination's,
//! a copy adopts each type its laid parts carry, and a value whose types its habitat cannot
//! resolve fails the debug habitat check.
//!
//! Every type here is interned in a cell, so it lands in that cell's branch rather than the
//! fixture's builder, which every cell's chain reads.

use std::collections::HashSet;

use crate::memory::{CellGraph, Header, ReleaseAbsorption};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeDigest, TypeNode};
use crate::values::{Key, TypeHeader, TypeValue, ValueFamily};

use super::{Dict, List, Record, TestGraph, Value, copy, pin, text, with_fixture};

/// The digests the branch behind `header` holds, or none where the half has no branch.
fn branch_digests(header: Header<'_, '_, '_, TypeHeader>) -> HashSet<TypeDigest> {
    let mut digests = HashSet::new();
    if let Some(branch) = header.get() {
        branch.each_digest(|digest| {
            digests.insert(digest);
        });
    }
    digests
}

#[test]
fn a_result_merges_its_homes_branch() {
    with_fixture(|fixture| {
        let scratch = fixture.scratch();
        let mut graph: TestGraph<'_> = CellGraph::new(2, pin);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let held = graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let words = [text(writer, "alpha")];
                let inner = List::new(writer, words.into_iter(), types, scratch);
                let entries = [(Key::str("key"), Value::List(inner))];
                let dict = Dict::new(writer, &entries, types, scratch);
                let source = context.lift::<ValueFamily>(Value::Dict(dict));
                let _ = fixture.tiers.cross(context, dest, &source).unwrap();
                branch_digests(context.header())
            })
            .unwrap();
        assert!(
            held.len() >= 2,
            "the home's branch holds the list's type and the dict's"
        );
        let merged = graph
            .enter(dest, |context| branch_digests(context.header()))
            .unwrap();
        assert_eq!(
            merged, held,
            "one pinned result into an empty destination leaves it holding the home's branch"
        );
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_copy_lays_each_seen_part_memo_and_nothing_hidden() {
    with_fixture(|fixture| {
        let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
        let field = |name| BinderSymbol::declared(name, symbols).unwrap();
        let (x, y) = (field("x"), field("y"));
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let [shown, seen_part, hidden_part, hidden_whole] = graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let numbers = [Value::Number(1.0)];
                let seen = List::new(writer, numbers.into_iter(), types, scratch);
                let entries = [(Key::str("key"), Value::Number(2.0))];
                let hidden = Dict::new(writer, &entries, types, scratch);
                let fields = [(x, Value::List(seen)), (y, Value::Dict(hidden))];
                let whole = Record::new(writer, &fields, types, scratch);
                // The shown record names `x` at `Any`, so the list's own memo reaches the copy
                // only as the memo of the part it lays.
                let shown = types.record(scratch, &[(x, KType::ANY)]);
                let retyped = Value::Record(whole).retyped(writer, shown, types, scratch);
                let source = context.lift::<ValueFamily>(retyped);
                let _ = fixture.tiers.cross(context, dest, &source).unwrap();
                [shown, seen.ktype(), hidden.ktype(), whole.ktype()].map(KType::digest)
            })
            .unwrap();
        let adopted = graph
            .enter(dest, |context| branch_digests(context.header()))
            .unwrap();
        assert!(adopted.contains(&shown), "the copy's own type is adopted");
        assert!(
            adopted.contains(&seen_part),
            "a laid part's memo is adopted"
        );
        assert!(
            !adopted.contains(&hidden_part) && !adopted.contains(&hidden_whole),
            "a hidden part's memo, and the whole's memo that names it, are not"
        );
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_copied_type_value_names_a_type_its_destination_resolves() {
    with_fixture(|fixture| {
        let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
        let x = BinderSymbol::declared("x", symbols).unwrap();
        let mut graph: TestGraph<'_> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let named = graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let named = types.record(scratch, &[(x, KType::NUMBER)]);
                let value = Value::Type(TypeValue::new(writer, named, types));
                let source = context.lift::<ValueFamily>(value);
                let _ = fixture.tiers.cross(context, dest, &source).unwrap();
                named
            })
            .unwrap();
        // The home goes first, so the destination reads a node the copy laid, not the home's.
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                assert!(branch_digests(context.header()).contains(&named.digest()));
                let types = fixture.tiers.of(context);
                assert!(
                    matches!(types.node(named), TypeNode::Record { .. }),
                    "the destination reads the record the copy names"
                );
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

/// A value laid in one cell by a door handed another cell's chain carries a type only that other
/// cell's branch holds. Nothing crossed it, so the check over the destination's chain refuses it.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a crossed value carries a type its habitat cannot resolve")]
fn a_handle_laid_without_a_crossing_fails_the_habitat_check() {
    use crate::memory::Active;
    use crate::values::{NoKnot, check_habitat};
    with_fixture(|fixture| {
        let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
        let x = BinderSymbol::declared("x", symbols).unwrap();
        let mut graph: TestGraph<'_> = CellGraph::new(2, pin);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let types = &fixture.tiers.of(context);
                let named = types.record(scratch, &[(x, KType::NUMBER)]);
                context
                    .alloc_into::<ValueFamily, ValueFamily>(dest, &[], |writer, header, _| {
                        let value = Value::Type(TypeValue::new(writer, named, types));
                        check_habitat::<NoKnot>(&fixture.tiers.half(writer, header), &value);
                        Active::new(value)
                    })
                    .unwrap();
            })
            .unwrap();
    });
}
