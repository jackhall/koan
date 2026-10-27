//! The region header: a half that builds a chain lays no header until it interns, and its first
//! intern lays the record and the branch together ([header.rs](../header.rs)).

use std::ptr;

use crate::memory::{
    CellGraph, NoDelivery, NoScratch, Prices, ReleaseAbsorption, Verdict, reattachable,
};
use crate::scope::RegionHeader;
use crate::type_lattice::{Habitat, Own};
use crate::values::Half;

/// A continuation family for a graph whose cells only store.
struct Unit;
reattachable!(Unit => ());

fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

#[test]
fn a_region_header_is_laid_on_the_first_intern_and_holds_the_branch() {
    let mut graph: CellGraph<'static, Unit, NoScratch, NoDelivery, RegionHeader> =
        CellGraph::new(1, pin);
    let parent = graph.create(None).unwrap();
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
        })
        .unwrap();
    let kept = graph
        .enter(parent, |context| {
            let header = context
                .header()
                .get()
                .expect("the header outlives its step");
            header.branch().is_some()
        })
        .unwrap();
    assert!(kept);
    graph
        .release(parent, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}
