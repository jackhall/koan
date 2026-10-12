//! The tiers' own contract, which no law states: a branch's size when laid, the root's variance,
//! that a link is read once per chain and never by an intern, which tier keeps a verdict, and how
//! far a merge walks.

use crate::bump::Bump;
use crate::symbols::{BinderSymbol, SymbolInterner};

use crate::types::chain::TypeChain;
use crate::types::handle::{KType, TypeHandle};
use crate::types::tiers::{Branch, BumpHalf, Root};
use crate::types::typed::is_subtype_of;
use crate::types::verdicts::Relation;

/// A frozen root is covariant, so a chain holds it at its own lifetime.
#[allow(dead_code)]
fn a_root_shortens<'short, 'long: 'short>(root: &'long Root<'long>) -> &'short Root<'short> {
    root
}

#[test]
fn a_new_branch_holds_four_slots() {
    let bump = Bump::new();
    let branch = Branch::new(&bump);
    assert_eq!(branch.slots(), 4);
    assert!(branch.is_empty());
}

/// A record over a field named `name`: content the root does not hold.
fn record_named<'t>(
    types: &TypeChain<'t, BumpHalf<'t>>,
    symbols: &SymbolInterner,
    name: &str,
) -> KType {
    let field = BinderSymbol::declared(name, symbols).expect("a bindable token");
    types.record(&Bump::new(), &[(field, KType::NUMBER)])
}

#[test]
fn a_second_read_through_a_link_probes_no_link() {
    let symbols = SymbolInterner::new();
    let (root_region, home_region, region) = (Bump::new(), Bump::new(), Bump::new());
    let root = Root::seeded(&root_region);
    let home = TypeChain::new(root, BumpHalf::new(&home_region));
    let minted = record_named(&home, &symbols, "x");
    let home_branch = BumpHalf::branch_of(&home);
    let child = TypeChain::new(root, BumpHalf::new(&region));
    child.link(home_branch);
    let digest = minted.raw().digest();
    assert!(
        child.find(digest).is_none(),
        "the child's tiers lack it before the read"
    );
    child.node(minted);
    assert!(
        child.find(digest).is_some(),
        "the read cached it in the child's own tier"
    );
    assert_eq!(child.own_len(), 1);
    child.node(minted);
    assert_eq!(child.own_len(), 1, "the second read probes no link");
}

#[test]
fn an_intern_lays_content_its_link_holds() {
    let symbols = SymbolInterner::new();
    let (root_region, home_region, region) = (Bump::new(), Bump::new(), Bump::new());
    let root = Root::seeded(&root_region);
    let home = TypeChain::new(root, BumpHalf::new(&home_region));
    let minted = record_named(&home, &symbols, "x");
    let home_branch = BumpHalf::branch_of(&home);
    let child = TypeChain::new(root, BumpHalf::new(&region));
    child.link(home_branch);
    let (home_len, child_len) = (home_branch.len(), child.own_len());
    assert_eq!(
        record_named(&child, &symbols, "x"),
        minted,
        "one content is one handle"
    );
    assert_eq!(
        home_branch.len(),
        home_len,
        "the linked tier is not written"
    );
    assert_eq!(
        child.own_len(),
        child_len + 1,
        "the intern laid its own node"
    );
    assert!(!std::ptr::eq(
        child.find(minted.raw().digest()).expect("laid"),
        home_branch.find(minted.raw().digest()).expect("held"),
    ));
}

/// A spawner holding `{x :Number}`, and a child linked to it that minted `{y :Number}`.
#[test]
fn a_merge_enters_what_its_source_reaches_through_its_link() {
    let symbols = SymbolInterner::new();
    let regions = [Bump::new(), Bump::new(), Bump::new(), Bump::new()];
    let root = Root::seeded(&regions[0]);
    let spawner = TypeChain::new(root, BumpHalf::new(&regions[1]));
    let inherited = record_named(&spawner, &symbols, "x");
    let child = TypeChain::new(root, BumpHalf::new(&regions[2]));
    child.link(BumpHalf::branch_of(&spawner));
    let minted = record_named(&child, &symbols, "y");

    let unlinked = TypeChain::new(root, BumpHalf::new(&regions[3]));
    unlinked.merge(BumpHalf::branch_of(&child));
    for kt in [minted, inherited] {
        assert!(
            unlinked.find(kt.raw().digest()).is_some(),
            "a destination that reaches neither branch enters both"
        );
    }
    assert_eq!(unlinked.own_len(), 2);

    // The spawner reaches itself, so a merge into it walks the child's branch alone.
    let spawner_len = spawner.own_len();
    spawner.merge(BumpHalf::branch_of(&child));
    assert_eq!(
        spawner.own_len(),
        spawner_len + 1,
        "only the child's mint is entered"
    );
}

/// The reach steps a merge into `into` takes, and how many nodes it enters.
fn merged<'t, 's: 't>(into: &TypeChain<'t, BumpHalf<'t>>, source: &Branch<'s>) -> (usize, usize) {
    let (steps, len) = (
        into.half().branch().map_or(0, Branch::reach_steps),
        into.own_len(),
    );
    into.merge(source);
    (
        BumpHalf::branch_of(into).reach_steps() - steps,
        into.own_len() - len,
    )
}

/// A merge walks the destination's chain only as far as the depth of the branch it checks, so a
/// destination 64 links deep checks as many steps as one 4 deep.
#[test]
fn a_merge_checks_as_many_steps_however_deep_its_destination() {
    let symbols = SymbolInterner::new();
    for depth in [4, 64] {
        let regions: Vec<Bump> = (0..depth + 6).map(|_| Bump::new()).collect();
        let root = Root::seeded(&regions[0]);
        let mut line: Vec<TypeChain<'_, BumpHalf<'_>>> = Vec::new();
        for (level, region) in regions[1..=depth + 1].iter().enumerate() {
            let chain = TypeChain::new(root, BumpHalf::new(region));
            if let Some(previous) = line.last() {
                chain.link(BumpHalf::branch_of(previous));
            }
            let _ = record_named(&chain, &symbols, &format!("level{level}"));
            line.push(chain);
        }
        let destination = &line[depth];
        assert_eq!(
            BumpHalf::branch_of(destination).position().depth as usize,
            depth
        );
        let linked = |region| {
            let chain = TypeChain::new(root, BumpHalf::new(region));
            chain.link(BumpHalf::branch_of(destination));
            chain
        };

        let child = linked(&regions[depth + 2]);
        let _ = record_named(&child, &symbols, "child");
        let into_child = merged(destination, BumpHalf::branch_of(&child));
        let into_link = merged(destination, BumpHalf::branch_of(&line[depth - 1]));
        let (sibling, other) = (linked(&regions[depth + 3]), linked(&regions[depth + 4]));
        let _ = record_named(&sibling, &symbols, "sibling");
        let _ = record_named(&other, &symbols, "other");
        let into_sibling = merged(&sibling, BumpHalf::branch_of(&other));
        let linkless = TypeChain::new(root, BumpHalf::new(&regions[depth + 5]));
        let _ = record_named(&linkless, &symbols, "linkless");
        let into_linkless = merged(destination, BumpHalf::branch_of(&linkless));

        assert_eq!(
            [into_child, into_link, into_sibling, into_linkless],
            [(2, 1), (2, 0), (3, 1), (1, 1)],
            "steps and entries merging a child, the link, a sibling and a linkless branch at depth {depth}"
        );
    }
}

/// A scratch chain reaches its region's branch, so a merge into it stops there.
#[test]
fn a_merge_into_a_scratch_habitat_stops_at_its_region() {
    let symbols = SymbolInterner::new();
    let (root_region, region, child_region, scratch_region) =
        (Bump::new(), Bump::new(), Bump::new(), Bump::new());
    let root = Root::seeded(&root_region);
    let region_half = BumpHalf::new(&region);
    let types = TypeChain::new(root, region_half);
    let _ = record_named(&types, &symbols, "region");
    let child = TypeChain::new(root, BumpHalf::new(&child_region));
    child.link(BumpHalf::branch_of(&types));
    let _ = record_named(&child, &symbols, "child");

    let in_scratch = TypeChain::scratch(root, region_half, BumpHalf::new(&scratch_region));
    assert_eq!(
        merged(&in_scratch, BumpHalf::branch_of(&child)),
        (3, 1),
        "the child's mint is entered, and the region's is not"
    );
}

#[test]
#[should_panic(expected = "a branch links as it is laid")]
fn a_laid_branch_takes_no_link() {
    let symbols = SymbolInterner::new();
    let (root_region, home_region, region) = (Bump::new(), Bump::new(), Bump::new());
    let root = Root::seeded(&root_region);
    let home = TypeChain::new(root, BumpHalf::new(&home_region));
    let _ = record_named(&home, &symbols, "x");
    let laid = TypeChain::new(root, BumpHalf::new(&region));
    let _ = record_named(&laid, &symbols, "y");
    laid.link(BumpHalf::branch_of(&home));
}

/// A birth links where the destination has laid no branch, and merges where it has.
#[test]
fn a_birth_links_its_home_unless_its_branch_is_laid() {
    let symbols = SymbolInterner::new();
    let regions = [Bump::new(), Bump::new(), Bump::new(), Bump::new()];
    let root = Root::seeded(&regions[0]);
    let host = TypeChain::new(root, BumpHalf::new(&regions[1]));
    let held = record_named(&host, &symbols, "x");

    let fresh = TypeChain::new(root, BumpHalf::new(&regions[2]));
    fresh.link_or_merge(BumpHalf::branch_of(&host));
    assert_eq!(fresh.own_len(), 0, "a link enters nothing");
    assert!(fresh.resolves(held), "and reaches the home's types");

    let laid = TypeChain::new(root, BumpHalf::new(&regions[3]));
    let _ = record_named(&laid, &symbols, "y");
    let laid_len = laid.own_len();
    laid.link_or_merge(BumpHalf::branch_of(&host));
    assert_eq!(
        laid.own_len(),
        laid_len + 1,
        "a laid branch merges the home"
    );

    // `fresh` links `host`, so `host` linking it back would close a cycle. `host` is laid, so it
    // merges, and the merge stops at its own branch.
    let _ = record_named(&fresh, &symbols, "z");
    let host_len = host.own_len();
    host.link_or_merge(BumpHalf::branch_of(&fresh));
    assert_eq!(
        host.own_len(),
        host_len + 1,
        "only the home's own mint is entered"
    );
}

#[test]
fn a_branch_verdict_is_recorded_in_the_branch() {
    let symbols = SymbolInterner::new();
    let (root_region, region, other_region, scratch) =
        (Bump::new(), Bump::new(), Bump::new(), Bump::new());
    let root = Root::seeded(&root_region);
    let types = TypeChain::new(root, BumpHalf::new(&region));
    let (x, y) = (
        record_named(&types, &symbols, "x"),
        record_named(&types, &symbols, "y"),
    );
    assert!(!is_subtype_of(&types, &scratch, x, y));
    let (x, y) = (x.raw().digest(), y.raw().digest());
    assert_eq!(types.verdict(x, y, Relation::Subtype), Some(false));
    let other = TypeChain::new(root, BumpHalf::new(&other_region));
    assert_eq!(
        other.verdict(x, y, Relation::Subtype),
        None,
        "released with the branch"
    );
}

#[test]
fn a_root_verdict_is_kept_by_the_root() {
    let (root_region, region, other_region, scratch) =
        (Bump::new(), Bump::new(), Bump::new(), Bump::new());
    let root = Root::seeded(&root_region);
    let types = TypeChain::new(root, BumpHalf::new(&region));
    assert!(is_subtype_of(
        &types,
        &scratch,
        KType::NUMBER,
        KType::ANY_VALUE
    ));
    let other = TypeChain::new(root, BumpHalf::new(&other_region));
    let (number, value) = (
        KType::NUMBER.raw().digest(),
        KType::ANY_VALUE.raw().digest(),
    );
    assert_eq!(other.verdict(number, value, Relation::Subtype), Some(true));
}

#[test]
fn a_scratch_chain_records_a_region_verdict_in_the_region() {
    let symbols = SymbolInterner::new();
    let (root_region, region, scratch_region, second_scratch, scratch) = (
        Bump::new(),
        Bump::new(),
        Bump::new(),
        Bump::new(),
        Bump::new(),
    );
    let root = Root::seeded(&root_region);
    let region_half = BumpHalf::new(&region);
    let types = TypeChain::new(root, region_half);
    let (x, y) = (
        record_named(&types, &symbols, "x"),
        record_named(&types, &symbols, "y"),
    );
    let in_scratch = TypeChain::scratch(root, region_half, BumpHalf::new(&scratch_region));
    assert!(!is_subtype_of(&in_scratch, &scratch, x, y));
    assert_eq!(
        in_scratch.own_len(),
        0,
        "the region's types are read, not laid"
    );
    let (x, y) = (x.raw().digest(), y.raw().digest());
    assert_eq!(types.verdict(x, y, Relation::Subtype), Some(false));
    let next = TypeChain::scratch(root, region_half, BumpHalf::new(&second_scratch));
    assert_eq!(next.verdict(x, y, Relation::Subtype), Some(false));
}
