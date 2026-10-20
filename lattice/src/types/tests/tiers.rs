//! The tiers' own contract, which no law states: a branch's size when laid, the root's variance,
//! that a link is read once per chain and never by an intern, where a late link is taken, which
//! tier keeps a verdict, and how far a merge walks.

use crate::bump::Bump;
use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol};

use crate::types::chain::TypeChain;
use crate::types::handle::{KType, TypeHandle};
use crate::types::kind::KKind;
use crate::types::render::display_name;
use crate::types::tiers::{Agent, Branch, BumpHalf, Frozen, Habitat, RootBuilder};
use crate::types::typed::is_subtype_of;
use crate::types::verdicts::Relation;
use crate::types::window::{RecursiveGroupWindow, RelativeSchema};

/// A frozen load is covariant, so a chain holds it at its own lifetime.
#[allow(dead_code)]
fn a_frozen_load_shortens<'short, 'long: 'short>(
    frozen: &'long Frozen<'long>,
) -> &'short Frozen<'short> {
    frozen
}

/// The names [`a_frozen_load_reads_alike_from_two_threads`] declares: a symbol is its text's
/// digest, so each thread's interner names the load's symbols alike.
fn declared(symbols: &SymbolInterner) -> ([BinderSymbol; 2], [TypeSymbol; 2]) {
    (
        ["x", "y"].map(|name| BinderSymbol::declared(name, symbols).expect("a bindable token")),
        ["Wrapper", "Other"].map(|name| TypeSymbol::declared(name, symbols).expect("a Type token")),
    )
}

/// What a reader of the frozen load answers for each of `handles`: its rendering, whether it is in
/// the load, and whether it reads a member schema.
fn read_load<'t>(
    types: &TypeChain<'t, BumpHalf<'t>>,
    symbols: &SymbolInterner,
    handles: &[KType],
) -> Vec<(String, bool, bool)> {
    handles
        .iter()
        .map(|&handle| {
            (
                display_name(handle, types, symbols).to_string(),
                types.in_root(handle),
                types.member_schema(handle).is_some(),
            )
        })
        .collect()
}

#[test]
fn a_frozen_load_reads_alike_from_two_threads() {
    let symbols = SymbolInterner::new();
    let ([x, y], [wrapper, other]) = declared(&symbols);
    let (load, host, scratch) = (Bump::new(), Bump::new(), Bump::new());
    let builder = RootBuilder::in_region(&load);
    let building = TypeChain::building(builder);
    let record = building.record(&scratch, &[(x, KType::NUMBER), (y, KType::STR)]);
    let union = building.union_of(&scratch, &[KType::NUMBER, KType::STR]);
    let function = building.function_type(&scratch, &[(x, record)], union);
    let window =
        RecursiveGroupWindow::new(&host, &[(wrapper, KKind::NewType), (other, KKind::NewType)]);
    let schema =
        RelativeSchema::NewType(building.list(window.sibling(other, KKind::NewType, &building)));
    assert!(window.fill_member(0, schema, &building, &scratch).is_none());
    let sealed = window
        .fill_member(1, schema, &building, &scratch)
        .expect("the last fill seals");
    let members = [0, 1].map(|index| sealed.member(index).expect("every member seals"));
    let handles = [record, union, function, members[0], members[1]];
    let frozen = builder.freeze();

    let here = Bump::new();
    let expected = read_load(
        &TypeChain::new(frozen, Agent::in_region(&here), BumpHalf::new(&here)),
        &symbols,
        &handles,
    );
    assert!(expected.iter().all(|&(_, in_load, _)| in_load));
    assert!(
        expected[3].2 && expected[4].2,
        "a sealed member reads its schema"
    );
    std::thread::scope(|threads| {
        let readers = [(); 2].map(|()| {
            threads.spawn(|| {
                let symbols = SymbolInterner::new();
                declared(&symbols);
                let half = Bump::new();
                read_load(
                    &TypeChain::new(frozen, Agent::in_region(&half), BumpHalf::new(&half)),
                    &symbols,
                    &handles,
                )
            })
        });
        for reader in readers {
            assert_eq!(reader.join().expect("a reader finishes"), expected);
        }
    });
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
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let home = TypeChain::new(root, agent, BumpHalf::new(&home_region));
    let minted = record_named(&home, &symbols, "x");
    let home_branch = BumpHalf::branch_of(&home);
    let child = TypeChain::new(root, agent, BumpHalf::new(&region));
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
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let home = TypeChain::new(root, agent, BumpHalf::new(&home_region));
    let minted = record_named(&home, &symbols, "x");
    let home_branch = BumpHalf::branch_of(&home);
    let child = TypeChain::new(root, agent, BumpHalf::new(&region));
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
    let root = Frozen::seeded(&regions[0]);
    let agent = Agent::in_region(&regions[0]);
    let spawner = TypeChain::new(root, agent, BumpHalf::new(&regions[1]));
    let inherited = record_named(&spawner, &symbols, "x");
    let child = TypeChain::new(root, agent, BumpHalf::new(&regions[2]));
    child.link(BumpHalf::branch_of(&spawner));
    let minted = record_named(&child, &symbols, "y");

    let unlinked = TypeChain::new(root, agent, BumpHalf::new(&regions[3]));
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
fn merged<'t, 's: 't>(
    into: &TypeChain<'t, impl Habitat<'t>>,
    source: &Branch<'s>,
) -> (usize, usize) {
    let steps = || {
        into.half()
            .own()
            .map_or(0, |own| own.branch().reach_steps())
    };
    let (before, len) = (steps(), into.own_len());
    into.merge(source);
    (steps() - before, into.own_len() - len)
}

/// A merge walks the destination's chain only as far as the depth of the branch it checks, so a
/// destination 64 links deep checks as many steps as one 4 deep.
#[test]
fn a_merge_checks_as_many_steps_however_deep_its_destination() {
    let symbols = SymbolInterner::new();
    for depth in [4, 64] {
        let regions: Vec<Bump> = (0..depth + 6).map(|_| Bump::new()).collect();
        let root = Frozen::seeded(&regions[0]);
        let agent = Agent::in_region(&regions[0]);
        let mut line: Vec<TypeChain<'_, BumpHalf<'_>>> = Vec::new();
        for (level, region) in regions[1..=depth + 1].iter().enumerate() {
            let chain = TypeChain::new(root, agent, BumpHalf::new(region));
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
            let chain = TypeChain::new(root, agent, BumpHalf::new(region));
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
        let linkless = TypeChain::new(root, agent, BumpHalf::new(&regions[depth + 5]));
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
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let region_half = BumpHalf::new(&region);
    let types = TypeChain::new(root, agent, region_half);
    let _ = record_named(&types, &symbols, "region");
    let child = TypeChain::new(root, agent, BumpHalf::new(&child_region));
    child.link(BumpHalf::branch_of(&types));
    let _ = record_named(&child, &symbols, "child");

    let in_scratch = TypeChain::scratch(root, agent, region_half, BumpHalf::new(&scratch_region));
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
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let home = TypeChain::new(root, agent, BumpHalf::new(&home_region));
    let _ = record_named(&home, &symbols, "x");
    let laid = TypeChain::new(root, agent, BumpHalf::new(&region));
    let _ = record_named(&laid, &symbols, "y");
    laid.link(BumpHalf::branch_of(&home));
}

/// A branch laid and holding a node, linked late to a home holding another, resolves both by
/// handle; its own node stays its own.
#[test]
fn a_late_link_reads_through_to_its_home() {
    let symbols = SymbolInterner::new();
    let (root_region, home_region, region) = (Bump::new(), Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let home = TypeChain::new(root, agent, BumpHalf::new(&home_region));
    let held = record_named(&home, &symbols, "x");
    let laid = TypeChain::new(root, agent, BumpHalf::new(&region));
    let own = record_named(&laid, &symbols, "y");
    assert!(!laid.resolves(held), "nothing links the home yet");
    let laid_len = laid.own_len();

    laid.link_late(BumpHalf::branch_of(&home));
    assert_eq!(laid.own_len(), laid_len, "a late link enters nothing");
    assert!(laid.resolves(held));
    assert!(laid.resolves(own));
}

/// A late link to the home a branch already links changes nothing; an unlaid half is linked as
/// `link` links it.
#[test]
fn a_late_link_to_the_home_it_links_changes_nothing() {
    let symbols = SymbolInterner::new();
    let (root_region, home_region, region) = (Bump::new(), Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let home = TypeChain::new(root, agent, BumpHalf::new(&home_region));
    let held = record_named(&home, &symbols, "x");
    let child = TypeChain::new(root, agent, BumpHalf::new(&region));
    child.link_late(BumpHalf::branch_of(&home));
    assert!(child.resolves(held));
    let _ = record_named(&child, &symbols, "y");
    let child_len = child.own_len();
    child.link_late(BumpHalf::branch_of(&home));
    assert_eq!(child.own_len(), child_len);
    assert!(child.resolves(held));
}

#[test]
#[should_panic(expected = "a branch another links is linked late")]
fn a_late_link_is_refused_where_another_branch_links_the_branch() {
    let symbols = SymbolInterner::new();
    let regions = [Bump::new(), Bump::new(), Bump::new(), Bump::new()];
    let root = Frozen::seeded(&regions[0]);
    let agent = Agent::in_region(&regions[0]);
    let home = TypeChain::new(root, agent, BumpHalf::new(&regions[1]));
    let _ = record_named(&home, &symbols, "x");
    let laid = TypeChain::new(root, agent, BumpHalf::new(&regions[2]));
    let _ = record_named(&laid, &symbols, "y");
    let linking = TypeChain::new(root, agent, BumpHalf::new(&regions[3]));
    linking.link(BumpHalf::branch_of(&laid));
    laid.link_late(BumpHalf::branch_of(&home));
}

/// A branch reached through links is one some branch links, which the flag refuses first; what
/// is left to the cycle check is a branch linked to itself.
#[test]
#[should_panic(expected = "a late link closes a cycle")]
fn a_late_link_is_refused_where_the_home_reaches_the_branch() {
    let symbols = SymbolInterner::new();
    let (root_region, region) = (Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let laid = TypeChain::new(root, agent, BumpHalf::new(&region));
    let _ = record_named(&laid, &symbols, "x");
    laid.link_late(BumpHalf::branch_of(&laid));
}

/// A birth links where the destination has laid no branch, and merges where it has.
#[test]
fn a_birth_links_its_home_unless_its_branch_is_laid() {
    let symbols = SymbolInterner::new();
    let regions = [Bump::new(), Bump::new(), Bump::new(), Bump::new()];
    let root = Frozen::seeded(&regions[0]);
    let agent = Agent::in_region(&regions[0]);
    let host = TypeChain::new(root, agent, BumpHalf::new(&regions[1]));
    let held = record_named(&host, &symbols, "x");

    let fresh = TypeChain::new(root, agent, BumpHalf::new(&regions[2]));
    fresh.link_or_merge(BumpHalf::branch_of(&host));
    assert_eq!(fresh.own_len(), 0, "a link enters nothing");
    assert!(fresh.resolves(held), "and reaches the home's types");

    let laid = TypeChain::new(root, agent, BumpHalf::new(&regions[3]));
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
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let types = TypeChain::new(root, agent, BumpHalf::new(&region));
    let (x, y) = (
        record_named(&types, &symbols, "x"),
        record_named(&types, &symbols, "y"),
    );
    assert!(!is_subtype_of(&types, &scratch, x, y));
    let (x, y) = (x.raw(), y.raw());
    assert_eq!(types.verdict(x, y, Relation::Subtype), Some(false));
    let other = TypeChain::new(root, agent, BumpHalf::new(&other_region));
    assert_eq!(
        other.verdict(x, y, Relation::Subtype),
        None,
        "released with the branch"
    );
}

#[test]
fn a_load_verdict_is_kept_by_the_load() {
    let (root_region, region, scratch) = (Bump::new(), Bump::new(), Bump::new());
    let builder = RootBuilder::in_region(&root_region);
    let building = TypeChain::building(builder);
    assert!(is_subtype_of(
        &building,
        &scratch,
        KType::NUMBER,
        KType::ANY_VALUE
    ));
    let types = TypeChain::new(
        builder.freeze(),
        Agent::in_region(&region),
        BumpHalf::new(&region),
    );
    let (number, value) = (KType::NUMBER.raw(), KType::ANY_VALUE.raw());
    assert_eq!(types.verdict(number, value, Relation::Subtype), Some(true));
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
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let region_half = BumpHalf::new(&region);
    let types = TypeChain::new(root, agent, region_half);
    let (x, y) = (
        record_named(&types, &symbols, "x"),
        record_named(&types, &symbols, "y"),
    );
    let in_scratch = TypeChain::scratch(root, agent, region_half, BumpHalf::new(&scratch_region));
    assert!(!is_subtype_of(&in_scratch, &scratch, x, y));
    assert_eq!(
        in_scratch.own_len(),
        0,
        "the region's types are read, not laid"
    );
    let (x, y) = (x.raw(), y.raw());
    assert_eq!(types.verdict(x, y, Relation::Subtype), Some(false));
    let next = TypeChain::scratch(root, agent, region_half, BumpHalf::new(&second_scratch));
    assert_eq!(next.verdict(x, y, Relation::Subtype), Some(false));
}

/// An adopt from a whole chain reads every tier the chain reads: a list laid in a scratch chain's
/// own tier over a record its region tier holds arrives in an unrelated chain in one pass.
#[test]
fn an_adopt_from_a_chain_reads_its_region_and_own_tiers() {
    let symbols = SymbolInterner::new();
    let (root_region, region, scratch_region, there_region) =
        (Bump::new(), Bump::new(), Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let region_half = BumpHalf::new(&region);
    let types = TypeChain::new(root, agent, region_half);
    let inner = record_named(&types, &symbols, "x");
    let in_scratch = TypeChain::scratch(root, agent, region_half, BumpHalf::new(&scratch_region));
    let outer = in_scratch.list(inner);
    assert_eq!(
        in_scratch.own_len(),
        1,
        "only the list is laid in the scratch"
    );

    let there = TypeChain::new(root, agent, BumpHalf::new(&there_region));
    there.adopt_from(&in_scratch, outer);
    assert_eq!(there.list(inner), outer, "it resolves with the same handle");
    assert!(there.find(inner.raw().digest()).is_some());
    assert_eq!(there.own_len(), 2);
}

/// An adopt from a chain reads its own branch's link, without caching into the source.
#[test]
fn an_adopt_from_a_chain_reads_through_its_link() {
    let symbols = SymbolInterner::new();
    let (root_region, home_region, region, there_region) =
        (Bump::new(), Bump::new(), Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let home = TypeChain::new(root, agent, BumpHalf::new(&home_region));
    let inner = record_named(&home, &symbols, "x");
    let child = TypeChain::new(root, agent, BumpHalf::new(&region));
    child.link(BumpHalf::branch_of(&home));
    let _ = record_named(&child, &symbols, "y");
    let child_len = child.own_len();

    let there = TypeChain::new(root, agent, BumpHalf::new(&there_region));
    there.adopt_from(&child, inner);
    assert!(there.find(inner.raw().digest()).is_some());
    assert_eq!(child.own_len(), child_len, "the source caches nothing");
}

#[test]
fn in_root_finds_a_root_type_and_not_a_branch_type() {
    let symbols = SymbolInterner::new();
    let (root_region, region) = (Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let types = TypeChain::new(root, agent, BumpHalf::new(&region));
    let minted = record_named(&types, &symbols, "x");
    assert!(types.in_root(KType::NUMBER), "a constant is the root's");
    assert!(types.resolves(minted));
    assert!(!types.in_root(minted), "a branch's mint is not the root's");
}

#[test]
fn in_root_on_the_loads_chain_reads_the_builder() {
    let symbols = SymbolInterner::new();
    let region = Bump::new();
    let types = crate::types::tests::building(&region);
    let field = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let minted = types.record(&Bump::new(), &[(field, KType::NUMBER)]);
    assert!(types.in_root(KType::NUMBER));
    assert!(
        types.in_root(minted),
        "the load's chain interns into the root"
    );
}

/// A branch handle's in-root bit is clear, so its lookup never probes the root; a root handle's
/// probes the root alone.
#[test]
fn a_branch_handle_is_looked_up_past_the_root() {
    let symbols = SymbolInterner::new();
    let region = Bump::new();
    let types = crate::types::tests::tiered(&region);
    let minted = record_named(&types, &symbols, "x");
    let before = types.load_probes();
    types.node(minted);
    assert_eq!(
        types.load_probes(),
        before,
        "a branch handle skips the root"
    );
    types.node(KType::NUMBER);
    assert_eq!(
        types.load_probes(),
        before + 1,
        "a root handle probes it once"
    );
}

#[test]
fn in_root_and_concrete_read_the_handle_alone() {
    let symbols = SymbolInterner::new();
    let (root_region, region) = (Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let types = TypeChain::new(root, agent, BumpHalf::new(&region));
    let minted = record_named(&types, &symbols, "x");
    let variable = types.quantified(0, KType::ANY);
    let (lookups, probes) = (types.lookups(), types.load_probes());
    let answers = [
        (
            types.in_root(KType::NUMBER),
            types.concrete(KType::NUMBER.into()).is_some(),
        ),
        (
            types.in_root(minted),
            types.concrete(minted.into()).is_some(),
        ),
        (types.in_root(variable), types.concrete(variable).is_some()),
    ];
    assert_eq!(types.lookups(), lookups, "no handle is entered");
    assert_eq!(types.load_probes(), probes, "no root table is probed");
    let truths = [KType::NUMBER.raw(), minted.raw(), variable.raw()].map(|handle| {
        (
            root.find(handle.digest()).is_some(),
            types.enter(handle).is_concrete(),
        )
    });
    assert_eq!(answers, truths);
}

#[test]
fn adopting_a_root_handle_lays_and_probes_nothing() {
    let (root_region, source_region, region) = (Bump::new(), Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let source = TypeChain::new(root, agent, BumpHalf::new(&source_region));
    let types = TypeChain::new(root, agent, BumpHalf::new(&region));
    types.adopt(None, KType::NUMBER);
    types.adopt_from(&source, KType::LIST_OF_ANY);
    assert_eq!(types.own_len(), 0);
    assert_eq!(types.load_probes(), 0);
}

#[test]
fn an_adopt_hits_where_the_chain_resolves_and_lays_where_it_lacks() {
    use crate::types::{ADOPT_HITS, ADOPT_LAID};
    let symbols = SymbolInterner::new();
    let (root_region, source_region, region) = (Bump::new(), Bump::new(), Bump::new());
    let root = Frozen::seeded(&root_region);
    let agent = Agent::in_region(&root_region);
    let source = TypeChain::new(root, agent, BumpHalf::new(&source_region));
    let types = TypeChain::new(root, agent, BumpHalf::new(&region));
    let minted = record_named(&source, &symbols, "x");
    assert!(!types.in_root(minted), "the seeded root lacks the record");
    ADOPT_HITS.set(0);
    ADOPT_LAID.set(0);
    types.adopt_from(&source, minted);
    assert_eq!(
        ADOPT_LAID.get(),
        1,
        "the first adopt lays the record's node"
    );
    let hits = ADOPT_HITS.get();
    types.adopt_from(&source, minted);
    assert_eq!(ADOPT_HITS.get(), hits + 1, "the second adopt hits at once");
    assert_eq!(ADOPT_LAID.get(), 1, "and lays nothing");
}

/// A record over a field named `name`, interned through `types`, whatever its habitat.
fn record_in<'t, H: Habitat<'t>>(
    types: &TypeChain<'t, H>,
    symbols: &SymbolInterner,
    name: &str,
) -> KType {
    let field = BinderSymbol::declared(name, symbols).expect("a bindable token");
    types.record(&Bump::new(), &[(field, KType::NUMBER)])
}

#[test]
fn an_agent_tier_hands_out_nodes_that_never_move() {
    let symbols = SymbolInterner::new();
    let (load, tier) = (Bump::new(), Bump::new());
    let agent = Agent::in_region(&tier);
    let resting = TypeChain::resting(Frozen::seeded(&load), agent);
    let first = record_in(&resting, &symbols, "x");
    assert!(!resting.in_root(first), "an agent node is no load node");
    let node = resting.locate(first.raw()).expect("the agent holds it");
    let mut nested = KType::NUMBER;
    for _ in 0..1000 {
        nested = resting.list(nested);
    }
    assert!(agent.len() > 1000, "the index grew past its first run");
    let found = resting.locate(first.raw()).expect("the agent holds it");
    assert!(std::ptr::eq(found, node));
}

#[test]
fn a_region_node_references_an_agent_node() {
    let symbols = SymbolInterner::new();
    let (load, tier, region) = (Bump::new(), Bump::new(), Bump::new());
    let (frozen, agent) = (Frozen::seeded(&load), Agent::in_region(&tier));
    let resting = TypeChain::resting(frozen, agent);
    let record = record_in(&resting, &symbols, "x");
    let types = TypeChain::new(frozen, agent, BumpHalf::new(&region));
    let list = types.list(record);
    let crate::types::stored::Content::List { element, .. } = types.enter(list.raw()).content()
    else {
        unreachable!("a list's node holds a list");
    };
    let in_agent = resting
        .locate(record.raw())
        .expect("the agent holds the record");
    assert!(std::ptr::eq(element, in_agent));
    assert_eq!(
        BumpHalf::branch_of(&types).len(),
        1,
        "the branch holds the list alone"
    );
}

#[test]
fn an_intern_finds_the_agent_before_the_branch() {
    let symbols = SymbolInterner::new();
    let (load, tier, region) = (Bump::new(), Bump::new(), Bump::new());
    let (frozen, agent) = (Frozen::seeded(&load), Agent::in_region(&tier));
    let rested = record_in(&TypeChain::resting(frozen, agent), &symbols, "x");
    let types = TypeChain::new(frozen, agent, BumpHalf::new(&region));
    assert_eq!(record_in(&types, &symbols, "x"), rested);
    assert_eq!(types.own_len(), 0, "the region lays nothing");
}

#[test]
fn a_run_verdict_over_two_load_types_lands_in_the_agent() {
    let (load, tier, region, other, scratch) = (
        Bump::new(),
        Bump::new(),
        Bump::new(),
        Bump::new(),
        Bump::new(),
    );
    let (frozen, agent) = (Frozen::seeded(&load), Agent::in_region(&tier));
    let types = TypeChain::new(frozen, agent, BumpHalf::new(&region));
    let (number, string) = (KType::NUMBER.raw(), KType::STR.raw());
    assert_eq!(types.verdict(number, string, Relation::Subtype), None);
    assert!(!is_subtype_of(&types, &scratch, KType::NUMBER, KType::STR));
    assert_eq!(agent.verdict_count(), 1);
    assert_eq!(types.own_len(), 0, "the region lays no branch");
    let fresh = TypeChain::new(frozen, agent, BumpHalf::new(&other));
    assert_eq!(
        fresh.verdict(number, string, Relation::Subtype),
        Some(false)
    );
}

#[test]
fn a_verdict_over_a_load_and_an_agent_type_lands_in_the_agent() {
    let symbols = SymbolInterner::new();
    let (load, tier, region, scratch) = (Bump::new(), Bump::new(), Bump::new(), Bump::new());
    let (frozen, agent) = (Frozen::seeded(&load), Agent::in_region(&tier));
    let record = record_in(&TypeChain::resting(frozen, agent), &symbols, "x");
    let types = TypeChain::new(frozen, agent, BumpHalf::new(&region));
    assert!(!is_subtype_of(&types, &scratch, record, KType::NUMBER));
    assert_eq!(agent.verdict_count(), 1);
    assert_eq!(types.own_len(), 0, "the region lays no branch");
}

#[test]
fn a_verdict_over_an_agent_type_and_a_branch_type_lands_in_the_branch() {
    let symbols = SymbolInterner::new();
    let (load, tier, region, scratch) = (Bump::new(), Bump::new(), Bump::new(), Bump::new());
    let (frozen, agent) = (Frozen::seeded(&load), Agent::in_region(&tier));
    let rested = record_in(&TypeChain::resting(frozen, agent), &symbols, "x");
    let types = TypeChain::new(frozen, agent, BumpHalf::new(&region));
    let minted = record_in(&types, &symbols, "y");
    assert!(!is_subtype_of(&types, &scratch, rested, minted));
    assert_eq!(agent.verdict_count(), 0);
    assert_eq!(BumpHalf::branch_of(&types).verdict_count(), 1);
}
