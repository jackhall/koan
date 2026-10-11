//! The tiers' own contract, which no law states: a branch's size when laid, the root's variance,
//! that a link is read once per chain and never by an intern, and which tier keeps a verdict.

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
