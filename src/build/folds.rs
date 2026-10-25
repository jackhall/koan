//! The load's **folds**: what the load decides per body once its walk has run, read off the
//! shapes and the walk's records alone ([the folds](README.md#the-folds)).
//! A resumption reads them and never recomputes them.
//!
//! - **Caching.** Each minting and composite site a body that may cache records is classed by what
//!   its content depends on: closed, invariant where it reads only parameters every tail edge into
//!   its frame passes on bare and that digest at a constant cost, else varying. The invariance and
//!   escape are the text's ([`Shapes::invariant`], [`Shapes::escapes`]); a site's pending paths are
//!   read through the records' resolutions. Each body's record says whether its frame caches,
//!   beside its classed sites and, on a callable's own, the parameters that may key it.
//! - **Keys.** A callable keys on each of its own keyable names that lies in the frontier of some
//!   unit beneath it, its entry aside, whose seam — the latest bind site among its frontier's
//!   unknowns — is a callable's entry, and on the parameters its invariant sites read. Each
//!   frontier's sites and keyable names are read once, through a memo shared by every frontier that
//!   reads it. The parameters such frontiers hold, keyable or not, are what a frame's or a block's
//!   entry binds in its activation where nothing settled them, a block's entry counting as a
//!   seam here, and the slots any body binds at a statement that any frontier holds are where the
//!   cell running it decides again.
//! - **Contract.** Each lexical variable a contribution or a solution names is composed by every
//!   callable or `MODULE` body between the body reading it and its home. The walk refuses a
//!   `MODULE` body composing a type its `OVER` list does not name.

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect, resident};
use crate::scope::{
    Address, BindSite, BodyClass, Caching, Cut, Deps, DepsGathering, Folds, Frontier, Keyed,
    LoadFacts, Mint, Position, Resolution, Resolved, Root, Segment, ShapeKind, Shapes, SiteDeps,
    Slot, UnitIndex, tree_deps,
};

/// The folds of every body `load` holds records for, by segment, laid through `lay`.
pub fn fold<'o>(
    load: &LoadFacts<'o>,
    lay: impl Lays<'o>,
    scratch: BumpAllocator<'_>,
) -> &'o [Folds<'o>] {
    let shapes = load.shapes();
    let count = shapes.bodies().len();
    // What the walk recorded, gathered from every unit's record.
    let mut sites: BumpVec<'_, SiteDeps<'o>> = BumpVec::new_in(scratch);
    let mut digestible: BumpVec<'_, (Segment, &'o [Slot])> = BumpVec::new_in(scratch);
    let mut resolutions: BumpVec<'_, ((Segment, usize), Resolution<'o>)> = BumpVec::new_in(scratch);
    let mut rooting: BumpVec<'_, (Segment, Slot)> = BumpVec::new_in(scratch);
    let mut frontiers: BumpVec<'_, (Segment, UnitIndex, Frontier<'o>)> = BumpVec::new_in(scratch);
    let mut contract = BumpVec::with_capacity_in(count, scratch);
    for shape in shapes.bodies() {
        let mut each = BumpVec::new_in(scratch);
        each.resize(shapes.layout(shape.segment()).len(), false);
        contract.push(each);
    }
    for (unit, (facts, frontier)) in load.own_units() {
        sites.extend(facts.sites.iter().copied());
        digestible.extend((facts.born.iter()).map(|(body, born)| (*body, born.digestible)));
        resolutions.extend((facts.paths.iter()).map(|(at, (_, resolution))| (*at, *resolution)));
        rooting.extend((facts.paths.iter()).filter_map(|(_, (entry, resolution))| {
            match (entry.root, resolution.resolved) {
                (Root::Binder(binder), Resolved::Class(_)) => Some((binder.body, binder.slot)),
                _ => None,
            }
        }));
        frontiers.push((unit.segment, unit.unit, *frontier));
        for named in facts.named_variables {
            compose(shapes, named.reading, named.level, &mut contract);
        }
    }
    digestible.sort_unstable_by_key(|(body, _)| *body);
    resolutions.sort_unstable_by_key(|(at, _)| *at);
    rooting.sort_unstable();
    rooting.dedup();
    let digestible = |body: Segment, slot: Slot| {
        (digestible.binary_search_by_key(&body, |(held, _)| *held))
            .is_ok_and(|at| digestible[at].1.binary_search(&slot).is_ok())
    };
    let keyable = |body: Segment, slot: Slot| {
        shapes.shape_at(body).position_of(slot) == Position::PARAMETER
            && digestible(body, slot)
            && shapes.rebound(body).binary_search(&slot).is_err()
    };
    let invariant_body = |frame: Segment| {
        shapes.shape_at(frame).kind() == ShapeKind::Callable && !shapes.escapes(frame)
    };
    // A site's class, beside the frame parameters an invariant one reads.
    let class_of = |site: &SiteDeps<'o>| -> (Mint, &'o [Slot]) {
        let mut deps = DepsGathering::new(scratch);
        deps.add(site.deps);
        for (root, level) in site.pending {
            match resolutions.binary_search_by_key(&(*root, *level), |(at, _)| *at) {
                Ok(at) => tree_deps(
                    shapes,
                    (site.holder, site.frame),
                    &resolutions[at].1.tree,
                    &mut deps,
                    scratch,
                ),
                Err(_) => deps.add(Deps::Varying),
            }
        }
        match deps.laid(lay) {
            Deps::Closed => (Mint::Closed, &[]),
            Deps::Varying => (Mint::Varying, &[]),
            Deps::On(slots) => {
                let held = slots.iter().all(|slot| {
                    invariant_body(site.frame)
                        && shapes.invariant(site.frame).binary_search(slot).is_ok()
                        && digestible(site.frame, *slot)
                });
                match held {
                    true => (Mint::Invariant, slots),
                    false => (Mint::Varying, &[]),
                }
            }
        }
    };
    // Per frame body, the parameters its invariant sites read, across every body it runs.
    let mut keyed = BumpVec::new_in(scratch);
    let mut classed = BumpVec::with_capacity_in(sites.len(), scratch);
    for site in sites.iter() {
        let (class, slots) = class_of(site);
        if class == Mint::Invariant {
            keyed.push((site.frame, None));
            keyed.extend(slots.iter().map(|slot| (site.frame, Some(*slot))));
        }
        classed.push((site.holder, site.site, site.minting, class));
    }
    keyed.sort_unstable();
    keyed.dedup();
    classed.sort_unstable_by_key(|(holder, ..)| *holder);
    let keyable = |body, at| matches!(at, Address::Slot(slot) if keyable(body, slot));
    let named = keyed_names(
        shapes,
        &frontiers,
        scratch,
        &keyable,
        &[ShapeKind::Callable],
    );
    // A quote's code's capture binds at its entry, as a callable's or a block's parameter does.
    let entry_bound = |body, at| match at {
        Address::Slot(slot) => shapes.shape_at(body).position_of(slot) == Position::PARAMETER,
        Address::Capture(_) => true,
    };
    let entered = [ShapeKind::Callable, ShapeKind::Block, ShapeKind::Code];
    let waiting = keyed_names(shapes, &frontiers, scratch, &entry_bound, &entered);
    let seaming = seam_names(shapes, &frontiers, scratch);
    let mut folds = BumpVec::with_capacity_in(count, scratch);
    for (at, shape) in shapes.bodies().iter().enumerate() {
        let segment = shape.segment();
        let frame = shapes.owner(segment);
        let contract = collect(lay, contract[at].iter().copied());
        // A quote's code that does not type has nothing an `EVAL` reads but its refusal.
        let closing = load
            .closing(frame)
            .filter(|(closing, _)| closing.refusal.is_none());
        let Some((closing, _)) = closing else {
            folds.push(Folds {
                caching: None,
                keys: None,
                waits: &[],
                seams: &[],
                rooting: &[],
                contract,
            });
            continue;
        };
        let caches = matches!(closing.class, Some(BodyClass::Sealed { caches: true, .. }));
        let (mut mints, mut warm) = (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
        let from = classed.partition_point(|(holder, ..)| *holder < segment);
        let to = classed.partition_point(|(holder, ..)| *holder <= segment);
        for (_, site, minting, class) in classed[from..to].iter() {
            match minting {
                true => mints.push((*site, *class)),
                false if *class == Mint::Invariant => warm.push(*site),
                false => {}
            }
        }
        let own = segment == frame;
        // The frame's run of `keyed`: its marker, then the parameters its invariant sites read.
        let start = keyed.partition_point(|(each, _)| *each < frame);
        let end = keyed.partition_point(|(each, _)| *each <= frame);
        let frame_keyed = keyed.get(start) == Some(&(frame, None));
        let invariant = (caches && own && frame_keyed).then(|| {
            let slots = keyed[start + 1..end]
                .iter()
                .map(|(_, slot)| slot.expect("sorted after the marker"));
            collect(lay, slots)
        });
        let callable = own && shape.kind() == ShapeKind::Callable;
        let caching = Caching {
            caches,
            mints: Keyed::sorted(lay, &mut mints),
            warm: Keyed::sorted(lay, &mut warm),
            invariant,
        };
        let keys = callable.then(|| {
            let from = named.partition_point(|(each, _)| *each < segment);
            let to = named.partition_point(|(each, _)| *each <= segment);
            let mut slots = BumpVec::new_in(scratch);
            slots.extend(named[from..to].iter().filter_map(|(_, at)| match at {
                Address::Slot(slot) => Some(*slot),
                Address::Capture(_) => None,
            }));
            slots.extend_from_slice(invariant.unwrap_or(&[]));
            slots.sort_unstable();
            slots.dedup();
            (invariant.is_some() || !slots.is_empty()).then(|| collect(lay, slots.iter().copied()))
        });
        let waits = match callable || matches!(shape.kind(), ShapeKind::Block | ShapeKind::Code) {
            true => {
                let from = waiting.partition_point(|(each, _)| *each < segment);
                let to = waiting.partition_point(|(each, _)| *each <= segment);
                collect(lay, waiting[from..to].iter().map(|(_, at)| *at))
            }
            false => &[],
        };
        let from = seaming.partition_point(|(each, _)| *each < segment);
        let to = seaming.partition_point(|(each, _)| *each <= segment);
        folds.push(Folds {
            caching: Some(resident(lay, caching)),
            keys: keys.flatten(),
            waits,
            seams: collect(lay, seaming[from..to].iter().map(|(_, slot)| *slot)),
            rooting: {
                let mut slots = BumpVec::new_in(scratch);
                slots.extend(
                    (seaming[from..to].iter())
                        .filter(|at| rooting.binary_search(at).is_ok())
                        .map(|(_, slot)| *slot),
                );
                collect(lay, slots.iter().copied())
            },
            contract,
        });
    }
    collect(lay, folds.iter().copied())
}

/// Each `(body, address)` a body of a kind in `entered` keys on through its frontiers: a `keyable`
/// name of it in the frontier of a unit beneath it, its entry aside, whose seam is the entry of a
/// body of such a kind. Sorted. Each frontier's sites and names are read once, through a memo
/// every frontier reading it shares.
pub(super) fn keyed_names<'x>(
    shapes: &Shapes<'_>,
    frontiers: &[(Segment, UnitIndex, Frontier<'_>)],
    scratch: BumpAllocator<'x>,
    keyable: &dyn Fn(Segment, Address) -> bool,
    entered: &[ShapeKind],
) -> BumpVec<'x, (Segment, Address)> {
    let mut summaries = bump_table(scratch);
    let mut named = BumpVec::new_in(scratch);
    for (segment, unit, frontier) in frontiers {
        // An entry holds its parameters' own types, which key nothing: whatever reads a parameter
        // waits on its names too.
        if *unit == UnitIndex::ENTRY && entered.contains(&shapes.shape_at(*segment).kind()) {
            continue;
        }
        let summary = summarize(shapes, *frontier, scratch, keyable, entered, &mut summaries);
        // A unit's own binders are what it fixes, not what it waits on: of each segment's two
        // latest distinct sites, at most one is the unit's.
        let Some(seam) = (summary.sites.iter())
            .filter_map(|(_, top)| {
                (top.iter().flatten())
                    .find(|site| (site.segment, site.unit) != (*segment, *unit))
                    .copied()
            })
            .max()
        else {
            continue;
        };
        if seam.unit != UnitIndex::ENTRY || !entered.contains(&shapes.shape_at(seam.segment).kind())
        {
            continue;
        }
        // Only an unknown a body around the unit declares: one off its chain is bound in a frame
        // of its own.
        named.extend(
            (summary.keyable.iter())
                .copied()
                .filter(|(declaring, _)| shapes.encloses(*declaring, *segment)),
        );
    }
    named.sort_unstable();
    named.dedup();
    named
}

/// Each `(body, slot)` bound at a statement of `body` that some unit's frontier holds: where the
/// cell running `body` decides again once the statement binds it. Sorted. Each frontier's
/// unknowns are laid flat once, through a memo every frontier reading it shares.
fn seam_names<'x>(
    shapes: &Shapes<'_>,
    frontiers: &[(Segment, UnitIndex, Frontier<'_>)],
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, (Segment, Slot)> {
    let mut flat: BumpBackedMap<'x, (usize, usize), &'x [(Segment, Slot)]> = bump_table(scratch);
    let mut named = BumpVec::new_in(scratch);
    for (segment, unit, frontier) in frontiers {
        let mut stack = BumpVec::new_in(scratch);
        stack.push((*frontier, false));
        while let Some((at, expanded)) = stack.pop() {
            if flat.contains_key(&at.identity()) {
                continue;
            }
            if !expanded {
                stack.push((at, true));
                stack.extend((at.reads().iter()).map(|cut| (cut.frontier, false)));
                continue;
            }
            let mut seams = BumpVec::new_in(scratch);
            seams.extend((at.own().iter()).filter_map(|unknown| match unknown.at {
                Address::Slot(slot)
                    if shapes.shape_at(unknown.segment).position_of(slot)
                        != Position::PARAMETER =>
                {
                    Some((unknown.segment, slot))
                }
                _ => None,
            }));
            for cut in at.reads() {
                let read = flat[&cut.frontier.identity()];
                seams.extend(read.iter().filter(|(segment, _)| *segment >= cut.from));
            }
            seams.sort_unstable();
            seams.dedup();
            flat.insert(at.identity(), collect(scratch, seams.iter().copied()));
        }
        // A unit's own binders are what it fixes, not what it waits on.
        named.extend(flat[&frontier.identity()].iter().filter(|(body, slot)| {
            let site = shapes.bind_site(crate::scope::Unknown {
                segment: *body,
                at: Address::Slot(*slot),
            });
            (site.segment, site.unit) != (*segment, *unit)
        }));
    }
    named.sort_unstable();
    named.dedup();
    named
}

/// What the keys fold reads off one frontier's unknowns: per body declaring some of them, by
/// segment ascending, the two latest distinct bind sites among them; and the keyable parameters of
/// callables among them, sorted. A cut at `from` keeps the entries at segments `>= from`, a suffix
/// of each, since every list here is sorted by segment first.
#[derive(Clone, Copy)]
struct Summary<'x> {
    sites: &'x [(Segment, [Option<BindSite>; 2])],
    keyable: &'x [(Segment, Address)],
}

/// The summary of `frontier`, and of every frontier it reads, memoized in `summaries` by
/// identity: its latest bind sites, and its `keyable` names of bodies of the kinds `entered`. Frontiers read only ended ones, so they form a DAG; a chain of k reads is k deep, so
/// the post-order keeps its own stack.
fn summarize<'x>(
    shapes: &Shapes<'_>,
    frontier: Frontier<'_>,
    scratch: BumpAllocator<'x>,
    keyable: &dyn Fn(Segment, Address) -> bool,
    entered: &[ShapeKind],
    summaries: &mut BumpBackedMap<'x, (usize, usize), Summary<'x>>,
) -> Summary<'x> {
    let mut stack = BumpVec::new_in(scratch);
    stack.push((frontier, false));
    while let Some((at, expanded)) = stack.pop() {
        if summaries.contains_key(&at.identity()) {
            continue;
        }
        if !expanded {
            stack.push((at, true));
            stack.extend((at.reads().iter()).map(|cut| (cut.frontier, false)));
            continue;
        }
        let read = |cut: &Cut<'_>| summaries[&cut.frontier.identity()];
        let mut sites = BumpVec::new_in(scratch);
        sites
            .extend((at.own().iter()).map(|unknown| (unknown.segment, shapes.bind_site(*unknown))));
        for cut in at.reads() {
            let held = read(cut).sites;
            let past = held.partition_point(|(segment, _)| *segment < cut.from);
            sites.extend((held[past..].iter()).flat_map(|(segment, top)| {
                top.iter().flatten().map(move |site| (*segment, *site))
            }));
        }
        sites.sort_unstable_by(|(mine, me), (theirs, them)| mine.cmp(theirs).then(them.cmp(me)));
        sites.dedup();
        let mut tops: BumpVec<'_, (Segment, [Option<BindSite>; 2])> = BumpVec::new_in(scratch);
        for (segment, site) in sites {
            match tops.last_mut() {
                Some((held, top)) if *held == segment => {
                    if top[1].is_none() {
                        top[1] = Some(site);
                    }
                }
                _ => tops.push((segment, [Some(site), None])),
            }
        }
        let mut keys = BumpVec::new_in(scratch);
        keys.extend((at.own().iter()).filter_map(|unknown| {
            let named = entered.contains(&shapes.shape_at(unknown.segment).kind())
                && keyable(unknown.segment, unknown.at);
            named.then_some((unknown.segment, unknown.at))
        }));
        for cut in at.reads() {
            let held = read(cut).keyable;
            let past = held.partition_point(|(segment, _)| *segment < cut.from);
            keys.extend_from_slice(&held[past..]);
        }
        keys.sort_unstable();
        keys.dedup();
        let summary = Summary {
            sites: collect(scratch, tops.iter().copied()),
            keyable: collect(scratch, keys.iter().copied()),
        };
        summaries.insert(at.identity(), summary);
    }
    summaries[&frontier.identity()]
}

/// Mark the lexical variable at `level`, read from `reading`, composed by each callable or
/// `MODULE` body between `reading` and its home.
fn compose(
    shapes: &Shapes<'_>,
    reading: Segment,
    level: usize,
    contract: &mut BumpVec<'_, BumpVec<'_, bool>>,
) {
    let mut at = reading;
    loop {
        if shapes.home(at, level).is_some() {
            return;
        }
        let shape = shapes.shape_at(at);
        if matches!(shape.kind(), ShapeKind::Callable | ShapeKind::Module) {
            let layout = shapes.layout(at);
            if let Some(index) = layout.iter().position(|(held, _)| *held == level) {
                contract[at.index() as usize][index] = true;
            }
        }
        match (shape.kind(), shapes.parent(at)) {
            (ShapeKind::Program | ShapeKind::Code, _) | (_, None) => return,
            (_, Some((parent, _))) => at = parent,
        }
    }
}
