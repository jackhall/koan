//! The load's **folds**: what the load decides per body once its walk has run, read off the
//! shapes and the walk's records alone ([the folds](README.md#the-folds)).
//!
//! - **Caching.** Each body's record says whether its frame caches, as its closing class has it.
//! - **Entry and seams.** The parameters a callable's, a block's or a quote's code's entry binds
//!   are those some unit beneath it holds in its frontier, its entry aside, whose seam — the latest
//!   bind site among its frontier's unknowns — is that entry; and the slots any body binds at a
//!   statement that any frontier holds are its statement seams. Each frontier's sites and names are
//!   read once, through a memo shared by every frontier that reads it.
//! - **Contract.** Each lexical variable a contribution or a solution names is composed by every
//!   callable, `MODULE` body or quote's code between the body reading it and its home. The walk
//!   refuses a `MODULE` body composing a type its `OVER` list does not name. A quote's code also
//!   composes each variable some unit inside it holds in its frontier: its frame reads through no
//!   activation around it, so its entry binds what its value carries.

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect, resident};
use crate::scope::{
    Address, BindSite, Binder, BinderAt, BodyClass, Caching, Cut, Flattened, Folds, Frontier,
    LoadFacts, Position, Resolved, Root, Segment, ShapeKind, Shapes, Site, Slot, UnitIndex,
    Unknown,
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
    let mut rooting: BumpVec<'_, (Segment, Slot)> = BumpVec::new_in(scratch);
    let mut frontiers: BumpVec<'_, (Segment, UnitIndex, Frontier<'o>)> = BumpVec::new_in(scratch);
    let mut contract = BumpVec::with_capacity_in(count, scratch);
    for shape in shapes.bodies() {
        let mut each = BumpVec::new_in(scratch);
        each.resize(shapes.layout(shape.segment()).len(), false);
        contract.push(each);
    }
    let mut binders: BumpVec<'_, BumpVec<'_, Unknown>> = BumpVec::with_capacity_in(count, scratch);
    binders.resize_with(count, || BumpVec::new_in(scratch));
    let mut held: BumpVec<'_, (Segment, Site)> = BumpVec::new_in(scratch);
    let mut flat = Flattened::new(scratch);
    for (unit, (facts, frontier)) in load.own_units() {
        for noted in flat.of(frontier) {
            let home = noted.unknown;
            match shapes.level(home.segment, home.at) {
                Some(level) => compose_code(shapes, unit.segment, level, &mut contract),
                None => capture_binder(shapes, unit.segment, home, &mut binders),
            }
        }
        // A part held in place is no seam: the run binds its held slot whatever its content.
        rooting.extend((facts.paths.iter()).filter_map(|(_, (entry, resolution))| {
            match (entry.root, resolution.resolved) {
                (
                    Root::Binder(Binder {
                        body,
                        at: BinderAt::Slot(slot),
                    }),
                    Resolved::Class(_),
                ) => Some((body, slot)),
                _ => None,
            }
        }));
        frontiers.push((unit.segment, unit.unit, frontier));
        for (segment, typed) in facts.typed.entries() {
            held.extend(
                (typed.held.iter())
                    .filter(|(site, _)| binds_held(shapes, *segment, *site))
                    .map(|(site, _)| (*segment, *site)),
            );
        }
        for named in facts.named_variables {
            compose(shapes, named.reading, named.level, &mut contract);
        }
    }
    for each in binders.iter_mut() {
        each.sort_unstable();
        each.dedup();
    }
    held.sort_unstable();
    held.dedup();
    let held_in = |segment: Segment| {
        let from = held.partition_point(|(each, _)| *each < segment);
        let to = held.partition_point(|(each, _)| *each <= segment);
        collect(lay, held[from..to].iter().map(|(_, site)| *site))
    };
    rooting.sort_unstable();
    rooting.dedup();
    // A quote's code's capture binds at its entry, as a callable's or a block's parameter does.
    let entry_bound = |body, at| match at {
        Address::Slot(slot) => shapes.shape_at(body).position_of(slot) == Position::PARAMETER,
        Address::Capture(_) => true,
        Address::Held(_) => false,
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
                waits: &[],
                seams: &[],
                rooting: &[],
                contract,
                binders: collect(lay, binders[segment.index() as usize].iter().copied()),
                held: held_in(segment),
            });
            continue;
        };
        let caches = matches!(closing.class, Some(BodyClass::Sealed { caches: true, .. }));
        let callable = segment == frame && shape.kind() == ShapeKind::Callable;
        let caching = Caching { caches };
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
            binders: collect(lay, binders[segment.index() as usize].iter().copied()),
            held: held_in(segment),
        });
    }
    collect(lay, folds.iter().copied())
}

/// Whether the part held in place at `site` of `segment` binds at a statement of its own: one in
/// a unit that runs in its owning body after the entry, a block's or a `MODULE` body's in the unit
/// of its encloser that runs it. A part in an entry's unit binds nowhere of its own.
fn binds_held(shapes: &Shapes<'_>, segment: Segment, site: Site) -> bool {
    let (mut owner, mut unit) = (segment, shapes.shape_at(segment).unit_of_site(site));
    while let ShapeKind::Block | ShapeKind::Module = shapes.shape_at(owner).kind() {
        let Some(running) = shapes.birth_unit(owner) else {
            break;
        };
        (owner, unit) = (running.segment, running.unit);
    }
    unit.run_index().is_some()
}

/// Each `(body, address)` of a body of a kind in `entered` that its entry binds: a `keyable` name
/// of it in the frontier of a unit beneath it, its entry aside, whose seam is the entry of a body
/// of such a kind. Sorted. Each frontier's sites and names are read once, through a memo every
/// frontier reading it shares.
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
        // An entry holds its parameters' own types, which it binds nothing past: whatever reads a
        // parameter waits on its names too.
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
/// cell running `body` binds again once the statement binds it. Sorted. Each frontier's
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
            seams.extend(at.own_unknowns().filter_map(|unknown| match unknown.at {
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

/// What the entry fold reads off one frontier's unknowns: per body declaring some of them, by
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
        sites.extend(
            at.own_unknowns()
                .map(|unknown| (unknown.segment, shapes.bind_site(unknown))),
        );
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
        keys.extend(at.own_unknowns().filter_map(|unknown| {
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

/// Mark the lexical variable at `level`, which a unit of `reading` holds in its frontier, composed
/// by each quote's code between `reading` and its home.
fn compose_code(
    shapes: &Shapes<'_>,
    reading: Segment,
    level: usize,
    contract: &mut BumpVec<'_, BumpVec<'_, bool>>,
) {
    let mut at = reading;
    while shapes.home(at, level).is_none() {
        if shapes.shape_at(at).kind() == ShapeKind::Code {
            let layout = shapes.layout(at);
            if let Some(index) = layout.iter().position(|(held, _)| *held == level) {
                contract[at.index() as usize][index] = true;
            }
        }
        let Some((parent, _)) = shapes.parent(at) else {
            return;
        };
        at = parent;
    }
}

/// Capture `binder`, a binder of a body around `reading` that `reading`'s records read through no
/// type capture, in each callable or `MODULE` body between them, so a frame of each can bind it.
/// None where a quote's code lies between, whose entry binds what its value carries, or where
/// `binder` lies on no body around `reading`.
fn capture_binder(
    shapes: &Shapes<'_>,
    reading: Segment,
    binder: Unknown,
    binders: &mut BumpVec<'_, BumpVec<'_, Unknown>>,
) {
    let mut between = smallvec::SmallVec::<[Segment; 4]>::new();
    let mut at = reading;
    while at != binder.segment {
        match shapes.shape_at(at).kind() {
            ShapeKind::Callable | ShapeKind::Module => between.push(at),
            ShapeKind::Code => return,
            ShapeKind::Block | ShapeKind::Program => {}
        }
        let Some((parent, _)) = shapes.parent(at) else {
            return;
        };
        at = parent;
    }
    for body in between {
        binders[body.index() as usize].push(binder);
    }
}

/// Mark the lexical variable at `level`, read from `reading`, composed by each callable, `MODULE`
/// body or quote's code between `reading` and its home.
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
        if matches!(
            shape.kind(),
            ShapeKind::Callable | ShapeKind::Module | ShapeKind::Code
        ) {
            let layout = shapes.layout(at);
            if let Some(index) = layout.iter().position(|(held, _)| *held == level) {
                contract[at.index() as usize][index] = true;
            }
        }
        match shapes.parent(at) {
            None => return,
            Some((parent, _)) => at = parent,
        }
    }
}
