//! The region header: a half that builds a chain lays no header until it interns, its first
//! intern lays the record and the branch together, and a build cell's table and a
//! settlement's home rest in the same record ([header.rs](../header.rs)).

use std::num::NonZeroUsize;
use std::ptr;

use crate::memory::{
    CellGraph, NoDelivery, NoScratch, Prices, ReleaseAbsorption, Verdict, reattachable,
};
use crate::scope::RegionHeader;
use crate::type_lattice::{Habitat, Own};
use crate::values::{Half, SpecHome};

/// A continuation family for a graph whose cells only store.
struct Unit;
reattachable!(Unit => ());

fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

#[test]
fn a_region_header_is_laid_on_the_first_intern_and_holds_the_table_and_the_home() {
    let mut graph: CellGraph<'static, Unit, NoScratch, NoDelivery, RegionHeader> =
        CellGraph::new(1, pin);
    let parent = graph.create(None).unwrap();
    let child = graph.create_tree(parent, None).unwrap();
    let two = NonZeroUsize::new(2).unwrap();
    graph
        .enter(parent, |context| {
            let half = Half::of(context.writer(), context.header());
            assert!(
                half.own().is_none(),
                "a half no door interned into has no branch"
            );
            assert!(context.header().get().is_none(), "nor any header");
            let Own::Branch(branch) = half.own_or_lay() else {
                panic!("a cell's half lays a branch");
            };
            let header = context
                .header()
                .get()
                .expect("the first intern lays the header");
            assert!(
                header.branch().is_some_and(|laid| ptr::eq(laid, branch)),
                "the header holds the branch the intern laid"
            );
            assert!(header.table().is_none(), "no table until one is laid");
            let table = header.lay_table(half, two, two);
            assert!(
                ptr::eq(header.lay_table(half, two, two), table),
                "a laid table is kept"
            );
            assert_eq!(header.home(), None);
            let home = SpecHome {
                me: child,
                parent: parent.into(),
            };
            header.set_home(home);
            assert_eq!(header.home(), Some(home));
        })
        .unwrap();
    let kept = graph
        .enter(parent, |context| {
            let header = context
                .header()
                .get()
                .expect("the header outlives its step");
            header.branch().is_some() && header.table().is_some()
        })
        .unwrap();
    assert!(kept);
    graph.release_tree(child).unwrap();
    graph
        .release(parent, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}
