//! Where a crossed value's types land: a pinned value's home branch merges into its destination's,
//! a copy adopts each type its laid parts carry, and a value whose types its habitat cannot
//! resolve fails the debug habitat check.
//!
//! Every type here is interned in a cell, so it lands in that cell's branch rather than the
//! fixture's builder, which every cell's chain reads.

use std::cell::RefCell;
use std::collections::HashSet;
use std::num::NonZeroUsize;

use crate::memory::{CellGraph, Header, ReleaseAbsorption};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeDigest, TypeNode};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{Decision, Half, Key, SpecKey, SpecTable, TypeValue, ValueFamily};

use super::{Dict, List, Record, TestGraph, TestHeader, Value, copy, pin, text, with_fixture};

/// The digests the branch behind `header` holds, or none where the half has no branch.
fn branch_digests(header: Header<'_, '_, '_, TestHeader>) -> HashSet<TypeDigest> {
    let mut digests = HashSet::new();
    if let Some(branch) = header.get().and_then(|slot| slot.get()) {
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

#[test]
fn a_table_decides_hits_free_entries_and_its_least_recently_born_victim() {
    with_fixture(|_| {
        let mut graph: TestGraph<'_> = CellGraph::new(1, pin);
        let parent = graph.create(None).unwrap();
        let a = graph.create_tree(parent, None).unwrap();
        let b = graph.create_tree(parent, None).unwrap();
        let c = graph.create_tree(parent, None).unwrap();
        let key = |at| SpecKey(DigestHasher::new(Tag::Number).count(at).finished());
        let two = NonZeroUsize::new(2).unwrap();
        let live = RefCell::new(HashSet::from([a, b, c]));
        let is_live = |handle| live.borrow().contains(&handle);
        graph
            .enter(parent, |context| {
                let table = SpecTable::lay(Half::of(context.writer(), context.header()), two, two);
                assert_eq!(table.nested(), two);
                assert_eq!(table.decide(key(1), is_live), Decision::Fill(0));
                table.fill(0, key(1), a);
                assert_eq!(table.decide(key(2), is_live), Decision::Fill(1));
                table.fill(1, key(2), b);
                assert_eq!(table.decide(key(1), is_live), Decision::Hit(a));
                assert_eq!(
                    table.decide(key(3), is_live),
                    Decision::Evict(1, b),
                    "`a`'s hit is the newer birth"
                );
                assert!(table.stamp(b), "a hop stamps its specialization's entry");
                assert_eq!(table.decide(key(3), is_live), Decision::Evict(0, a));
                assert!(!table.stamp(c), "no entry names `c`");
                live.borrow_mut().remove(&b);
                assert_eq!(
                    table.decide(key(2), is_live),
                    Decision::Fill(1),
                    "a stale entry is renewed in place"
                );
                assert_eq!(table.decide(key(3), is_live), Decision::Fill(1));
                assert_eq!(table.len(), 2, "a stale entry is still occupied");
            })
            .unwrap();
        for child in [a, b, c] {
            graph.release_tree(child).unwrap();
        }
        graph
            .release(parent, ReleaseAbsorption::IntoHolder)
            .unwrap();
        assert!(graph.is_empty());
    });
}
