//! The load's store: a write reads back beside its frontier, and the facts it lays find every fact
//! a record keys, each in the record of the unit that wrote it.

use crate::memory::{BumpVec, resident};
use crate::scope::tests::with_shapes;
use crate::scope::{
    Address, Binding, Frontier, Keyed, Site, Slot, Store, Typed, UnitFacts, UnitId, UnitIndex,
    Unknown, each_part,
};

use super::super::LoadStore;

/// Three top-level statements, each its own unit.
const SOURCE: &str = "LET a = 1\nLET b = (a + 2)\nPRINT b";

#[test]
fn a_write_reads_back_and_the_laid_facts_find_every_site_it_keys() {
    with_shapes(SOURCE, |shapes, writer, scratch, _| {
        let root = shapes.root();
        let store = LoadStore::new(writer, scratch, shapes, Binding::EMPTY);
        let unknown = Unknown {
            segment: root.segment(),
            at: Address::Slot(Slot(0)),
        };
        let frontier = Frontier::from_sorted(writer.fill(1, |_| unknown));
        let mut written = Vec::new();
        for (index, statement) in root.body().iter().enumerate() {
            let unit = UnitId {
                segment: root.segment(),
                unit: root.unit_of_statement(index),
            };
            let mut sites = BumpVec::new_in(scratch);
            each_part(statement, |part| sites.push(Site::of(part)));
            // Even statements settle their sites and odd ones mint at them, so a read tells which
            // table of which record answered.
            let sites = Keyed::sorted(writer, &mut sites);
            let typed = match index % 2 {
                0 => Typed {
                    settled: sites,
                    ..Typed::EMPTY
                },
                _ => Typed {
                    minting: sites,
                    ..Typed::EMPTY
                },
            };
            let mut typed = [(root.segment(), typed)];
            let facts = resident(
                writer,
                UnitFacts {
                    typed: Keyed::sorted(writer, &mut typed),
                    ..UnitFacts::EMPTY
                },
            );
            store.write(unit, facts, frontier);
            let (back, at) = store.fact(unit).expect("a write reads back");
            assert!(std::ptr::eq(back, facts) && at.identity() == frontier.identity());
            written.push((index, sites));
        }
        assert!(
            store
                .fact(UnitId {
                    segment: root.segment(),
                    unit: UnitIndex::ENTRY,
                })
                .is_none(),
            "a unit the walk did not run has no record"
        );
        let facts = store.facts(scratch);
        for (index, sites) in written {
            for site in sites.iter() {
                assert_eq!(facts.settled(root, *site), index % 2 == 0);
                assert_eq!(facts.mints_at(root, *site), index % 2 == 1);
            }
        }
    });
}
