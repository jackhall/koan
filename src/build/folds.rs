//! The load's **folds**: what the load decides per body once its walk has run, read off the
//! shapes and the walk's records alone ([the folds](../../roadmap/gradual-build/one-walk-build.md)).
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
//!   unknowns — is a callable's entry, and on the parameters its invariant sites read.
//! - **Contract.** Each lexical variable a contribution or a solution names is composed by every
//!   callable or `MODULE` body between the body reading it and its home. The walk refuses a
//!   `MODULE` body composing a type its `OVER` list does not name.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect, resident};
use crate::scope::{
    Address, BodyClass, Caching, Deps, Folds, Frontier, Keyed, LoadFacts, Mint, Position,
    Resolution, Segment, ShapeError, ShapeKind, Shapes, SiteDeps, Slot, UnitIndex, tree_deps,
};

/// The folds of every body `load` holds records for, by segment, laid through `lay`.
pub fn fold<'o>(
    load: &LoadFacts<'o>,
    lay: impl Lays<'o>,
    scratch: BumpAllocator<'_>,
) -> Result<&'o [Folds<'o>], ShapeError<'o>> {
    let shapes = load.shapes();
    let count = shapes.bodies().len();
    // What the walk recorded, gathered from every unit's record.
    let mut sites: BumpVec<'_, SiteDeps<'o>> = BumpVec::new_in(scratch);
    let mut digestible: BumpVec<'_, (Segment, &'o [Slot])> = BumpVec::new_in(scratch);
    let mut resolutions: BumpVec<'_, ((Segment, usize), Resolution<'o>)> = BumpVec::new_in(scratch);
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
        frontiers.push((unit.segment, unit.unit, *frontier));
        for named in facts.named_variables {
            compose(shapes, named.reading, named.level, &mut contract);
        }
    }
    digestible.sort_unstable_by_key(|(body, _)| *body);
    resolutions.sort_unstable_by_key(|(at, _)| *at);
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
        let mut deps = site.deps;
        for (root, level) in site.pending {
            let each = match resolutions.binary_search_by_key(&(*root, *level), |(at, _)| *at) {
                Ok(at) => tree_deps(
                    shapes,
                    (site.holder, site.frame),
                    &resolutions[at].1.tree,
                    lay,
                    scratch,
                ),
                Err(_) => Deps::Varying,
            };
            deps = deps.union(each, lay, scratch);
        }
        match deps {
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
    let named = keyed_names(shapes, &frontiers, scratch, &keyable);
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
        let frame_keyed = keyed.binary_search(&(frame, None)).is_ok();
        let invariant = (caches && own && frame_keyed).then(|| {
            let mut slots = BumpVec::new_in(scratch);
            slots.extend(
                (keyed.iter())
                    .filter(|(each, slot)| *each == frame && slot.is_some())
                    .filter_map(|(_, slot)| *slot),
            );
            collect(lay, slots.iter().copied())
        });
        let callable = own && shape.kind() == ShapeKind::Callable;
        let mut keyable_slots = BumpVec::new_in(scratch);
        if callable {
            keyable_slots.extend(
                (0..shape.slots() as u32)
                    .map(Slot)
                    .filter(|slot| keyable(segment, *slot)),
            );
        }
        let caching = Caching {
            caches,
            mints: Keyed::sorted(lay, &mut mints),
            warm: Keyed::sorted(lay, &mut warm),
            invariant,
            keyable: collect(lay, keyable_slots.iter().copied()),
            escapes: !callable || !invariant_body(frame),
        };
        let keys = callable.then(|| {
            let from = named.partition_point(|(each, _)| *each < segment);
            let to = named.partition_point(|(each, _)| *each <= segment);
            let mut slots = BumpVec::new_in(scratch);
            slots.extend(named[from..to].iter().map(|(_, slot)| *slot));
            slots.extend_from_slice(invariant.unwrap_or(&[]));
            slots.sort_unstable();
            slots.dedup();
            (invariant.is_some() || !slots.is_empty()).then(|| collect(lay, slots.iter().copied()))
        });
        folds.push(Folds {
            caching: Some(resident(lay, caching)),
            keys: keys.flatten(),
            contract,
        });
    }
    Ok(collect(lay, folds.iter().copied()))
}

/// Each `(callable, slot)` a callable keys on through its frontiers: a keyable name of it in the
/// frontier of a unit beneath it, its entry aside, whose seam is a callable's entry. Sorted.
fn keyed_names<'x>(
    shapes: &Shapes<'_>,
    frontiers: &[(Segment, UnitIndex, Frontier<'_>)],
    scratch: BumpAllocator<'x>,
    keyable: &dyn Fn(Segment, Slot) -> bool,
) -> BumpVec<'x, (Segment, Slot)> {
    let mut named = BumpVec::new_in(scratch);
    for (segment, unit, frontier) in frontiers {
        // A callable's entry holds its parameters' own types, which key nothing: whatever reads a
        // parameter waits on its names too.
        if *unit == UnitIndex::ENTRY && shapes.shape_at(*segment).kind() == ShapeKind::Callable {
            continue;
        }
        // A unit's own binders are what it fixes, not what it waits on.
        let Some(seam) = (frontier.unknowns().iter())
            .map(|unknown| shapes.bind_site(*unknown))
            .filter(|site| (site.segment, site.unit) != (*segment, *unit))
            .max()
        else {
            continue;
        };
        if seam.unit != UnitIndex::ENTRY
            || shapes.shape_at(seam.segment).kind() != ShapeKind::Callable
        {
            continue;
        }
        for unknown in frontier.unknowns() {
            let Address::Slot(slot) = unknown.at else {
                continue;
            };
            let declaring = shapes.shape_at(unknown.segment);
            // Only an unknown a body around the unit declares: one off its chain is bound in a
            // frame of its own.
            if declaring.kind() == ShapeKind::Callable
                && encloses(shapes, unknown.segment, *segment)
                && keyable(unknown.segment, slot)
            {
                named.push((unknown.segment, slot));
            }
        }
    }
    named.sort_unstable();
    named.dedup();
    named
}

/// Whether `outer` is `inner` or a body around it.
fn encloses(shapes: &Shapes<'_>, outer: Segment, mut inner: Segment) -> bool {
    loop {
        if inner == outer {
            return true;
        }
        match shapes.parent(inner) {
            Some((parent, _)) => inner = parent,
            None => return false,
        }
    }
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
        if shapes.homes(at).iter().any(|(held, _)| *held == level) {
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
