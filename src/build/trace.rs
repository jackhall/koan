//! The **trace**: per call site of a keyed body, every binding the load can name
//! ([the trace](README.md#the-trace)).
//!
//! A fold over the base walk's [`Traced`](crate::scope::Traced) records, which say what each call
//! site passes at each of its callee's slots: load content, or a slot of the site's own callable
//! passed on bare. A body's **traced slots** are its keys and each slot it passes on to a callee's
//! traced slot, found backward. Its **bindings** are tuples over them, found forward: the sites
//! whose traced slots are all content seed them, and each binding is pushed through the slot maps
//! of the sites in its body, repeats dropped, so a combination is named only where a chain of call
//! sites passes it. Content is only passed on, never built, so the fold ends with no depth bound,
//! and it reads each site once per binding of its caller.
//!
//! A body is **open** — its bindings unnamed — where its name escapes, where the text lists a call
//! into it that no record names (one the load resolves to no one callee), one in a quote's code,
//! an `EVAL`'s offer of it to code or one passing nothing at a traced slot, or where it has more
//! bindings than the limit; and so is every body it passes a slot on to. The cap is the load's
//! ([`CAP`]); the enumeration takes it as an argument. The text's call sites and escapes are the
//! shape table's ([`Shapes::calls_into`], [`Shapes::escapes`]). The traced slots start from the
//! keys fold's keys, and the trace lays them back as each body's keys: a callable keys on every
//! slot it passes on, a parameter its own tail rebinds included.
//!
//! Beside the traces it lays each owning body's **slot maps**: each call site the body owns,
//! outside a quote's code, that passes something the trace follows at every key of its callee,
//! with what it passes there in key order. A layer of the owning body lays each such site's request
//! from its slot map.

use crate::memory::{BumpAllocator, BumpVec, Lays, bump_set, collect};
use crate::scope::{
    CallTrace, Content, Fixed, Folds, Keyed, LoadFacts, Open, Segment, ShapeKind, Shapes, Site,
    Slot, SlotMap, Untraced,
};
use crate::source::SourceRef;

/// The cap on a body's bindings at load.
pub const CAP: usize = 8;

/// One body's traced slots, in slot order, and the bindings the text passes it.
#[derive(Clone, Copy, Debug)]
pub struct Body<'e> {
    pub slots: &'e [Slot],
    pub bindings: Bindings<'e>,
}

/// A body's bindings: each a tuple over its traced slots, sorted; or why they are open.
#[derive(Clone, Copy, Debug)]
pub enum Bindings<'e> {
    Named(&'e [&'e [Content]]),
    Open(Open),
}

/// One call site's record: where it sits, the callable owning it (`None` at the top level or in a
/// quote's code), where the call is written, and what it passes its callee.
#[derive(Clone, Copy)]
struct Record<'o> {
    at: Segment,
    site: Site,
    caller: Option<Segment>,
    call: SourceRef,
    callee: Segment,
    fixed: &'o [(Slot, Fixed)],
}

impl Record<'_> {
    /// What the site passes at its callee's `slot`, where it passes anything the trace follows.
    fn at(&self, slot: Slot) -> Option<Fixed> {
        let index = (self.fixed)
            .binary_search_by_key(&slot, |(at, _)| *at)
            .ok()?;
        Some(self.fixed[index].1)
    }
}

#[cfg(test)]
thread_local! {
    static EDGE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this thread's enumerations have read a site to push a binding through it.
#[cfg(test)]
pub(crate) fn edge_reads() -> usize {
    EDGE_READS.with(|reads| reads.get())
}

/// The enumeration's state, by segment: why each body is open, the bindings each holds in the
/// order found, beside a set of them, and the bindings still to push through their bodies' sites.
struct State<'e, 'x> {
    open: BumpVec<'x, Option<Open>>,
    held: BumpVec<'x, BumpVec<'x, &'e [Content]>>,
    seen: BumpVec<'x, crate::memory::BumpBackedSet<'x, &'e [Content]>>,
    queue: BumpVec<'x, (Segment, usize)>,
    stack: BumpVec<'x, (Segment, Open)>,
    limit: usize,
    scratch: BumpAllocator<'e>,
}

impl<'e> State<'e, '_> {
    /// Open `body` for `reason`, and every body it passes a slot on to: `edges` gives each callee
    /// a body passes a slot on to.
    fn close<I: Iterator<Item = Segment>>(
        &mut self,
        body: Segment,
        reason: Open,
        edges: &impl Fn(Segment) -> I,
    ) {
        self.stack.push((body, reason));
        while let Some((body, reason)) = self.stack.pop() {
            let at = body.index() as usize;
            if self.open[at].is_some() {
                continue;
            }
            self.open[at] = Some(reason);
            self.stack
                .extend(edges(body).map(|callee| (callee, Open::Fed)));
        }
    }

    /// Give `body` the binding `tuple`, where it is not open and does not hold it: past the limit,
    /// open it instead.
    fn add<I: Iterator<Item = Segment>>(
        &mut self,
        body: Segment,
        tuple: &[Content],
        edges: &impl Fn(Segment) -> I,
    ) {
        let at = body.index() as usize;
        if self.open[at].is_some() || self.seen[at].contains(tuple) {
            return;
        }
        if self.held[at].len() == self.limit {
            self.close(body, Open::Capped, edges);
            return;
        }
        let laid: &'e [Content] = collect(self.scratch, tuple.iter().copied());
        self.seen[at].insert(laid);
        self.held[at].push(laid);
        self.queue.push((body, self.held[at].len() - 1));
    }
}

/// The keys of the body at `segment`, where a call of it seeks a settlement: the keys fold's, but
/// none for a body deferred to run, whose call runs under its composition.
fn keys<'o>(shapes: &Shapes<'_>, folds: &[Folds<'o>], segment: Segment) -> Option<&'o [Slot]> {
    let keys = folds.get(segment.index() as usize)?.keys?;
    (!shapes.shape_at(segment).deferred()).then_some(keys)
}

/// Every record the base walk laid, sorted by site and with no site twice.
fn records<'o, 'x>(load: &LoadFacts<'o>, scratch: BumpAllocator<'x>) -> BumpVec<'x, Record<'o>> {
    let shapes = load.shapes();
    let mut records = BumpVec::new_in(scratch);
    for (_, (facts, _)) in load.own_units() {
        for (at, typed) in facts.typed.iter() {
            let owner = shapes.owner(*at);
            let caller = (shapes.shape_at(owner).kind() == ShapeKind::Callable).then_some(owner);
            for (site, traced) in typed.traced.iter() {
                records.push(Record {
                    at: *at,
                    site: *site,
                    caller,
                    call: traced.call,
                    callee: traced.callee,
                    fixed: traced.fixed,
                });
            }
        }
    }
    records.sort_by_key(|record| (record.at, record.site));
    records.dedup_by_key(|record| (record.at, record.site));
    records
}

/// Each body's traced slots and bindings, by segment, enumerated over `load`'s records with at
/// most `limit` bindings a body, laid in `scratch`.
pub fn bindings<'e>(
    load: &LoadFacts<'_>,
    folds: &[Folds<'_>],
    limit: usize,
    scratch: BumpAllocator<'e>,
) -> &'e [Body<'e>] {
    let records = records(load, scratch);
    enumerate(load.shapes(), folds, &records, limit, scratch)
}

/// [`bindings`] over `records`.
fn enumerate<'e>(
    shapes: &Shapes<'_>,
    folds: &[Folds<'_>],
    records: &[Record<'_>],
    limit: usize,
    scratch: BumpAllocator<'e>,
) -> &'e [Body<'e>] {
    let count = shapes.bodies().len();
    // The records into each callee, and out of each caller, by index into `records`.
    let mut into = BumpVec::with_capacity_in(records.len(), scratch);
    into.extend(0..records.len());
    into.sort_by_key(|at: &usize| records[*at].callee);
    let mut out = BumpVec::with_capacity_in(records.len(), scratch);
    out.extend((0..records.len()).filter(|at| records[*at].caller.is_some()));
    out.sort_by_key(|at: &usize| records[*at].caller);
    let into_of = |callee: Segment| {
        let from = into.partition_point(|at| records[*at].callee < callee);
        let to = into.partition_point(|at| records[*at].callee <= callee);
        &into[from..to]
    };
    let out_of = |caller: Segment| {
        let from = out.partition_point(|at| records[*at].caller < Some(caller));
        let to = out.partition_point(|at| records[*at].caller <= Some(caller));
        &out[from..to]
    };

    // Traced slots, backward from each body's keys.
    let mut slots: BumpVec<'_, BumpVec<'_, Slot>> = BumpVec::with_capacity_in(count, scratch);
    let mut work = BumpVec::new_in(scratch);
    for at in 0..count {
        let segment = shapes.bodies()[at].segment();
        let keys = keys(shapes, folds, segment).unwrap_or(&[]);
        let mut own = BumpVec::with_capacity_in(keys.len(), scratch);
        own.extend_from_slice(keys);
        work.extend(keys.iter().map(|slot| (segment, *slot)));
        slots.push(own);
    }
    while let Some((body, slot)) = work.pop() {
        for record in into_of(body).iter().map(|at| &records[*at]) {
            if let (Some(Fixed::Passed(from)), Some(caller)) = (record.at(slot), record.caller) {
                let theirs = &mut slots[caller.index() as usize];
                if !theirs.contains(&from) {
                    theirs.push(from);
                    work.push((caller, from));
                }
            }
        }
    }
    for own in slots.iter_mut() {
        own.sort_unstable();
    }
    let traced = |body: Segment| -> &[Slot] { &slots[body.index() as usize] };
    // Whether a record passes on a slot of its caller at some traced slot of its callee.
    let edge = |record: &Record<'_>| {
        (traced(record.callee).iter())
            .any(|slot| matches!(record.at(*slot), Some(Fixed::Passed(_))))
    };

    let mut state = State {
        open: BumpVec::with_capacity_in(count, scratch),
        held: BumpVec::with_capacity_in(count, scratch),
        seen: BumpVec::with_capacity_in(count, scratch),
        queue: BumpVec::new_in(scratch),
        stack: BumpVec::new_in(scratch),
        limit,
        scratch,
    };
    state.open.resize(count, None);
    for _ in 0..count {
        state.held.push(BumpVec::new_in(scratch));
        state.seen.push(bump_set(scratch));
    }
    let edges = |body: Segment| {
        (out_of(body).iter().map(|at| &records[*at]))
            .filter(|record| edge(record))
            .map(|record| record.callee)
    };

    // Open from the start: an escaping body, and one the text calls where no record traces it.
    for shape in shapes.bodies() {
        let body = shape.segment();
        let mine = traced(body);
        if mine.is_empty() {
            continue;
        }
        if shapes.escapes(body) {
            state.close(body, Open::Escapes, &edges);
            continue;
        }
        let into = into_of(body);
        let untraced = shapes.calls_into(body).iter().any(|call| {
            let recorded = records
                .binary_search_by_key(&(call.at, call.site), |record| (record.at, record.site))
                .is_ok_and(|at| records[at].callee == body);
            call.offered || !recorded || shapes.in_code(call.at)
        });
        let unfollowed =
            (into.iter()).any(|at| mine.iter().any(|slot| records[*at].at(*slot).is_none()));
        if untraced || unfollowed {
            state.close(body, Open::Fed, &edges);
        }
    }

    // Seed each body from the sites passing content at every traced slot, then push each binding
    // through the sites in its body, first in first out.
    let mut tuple = BumpVec::new_in(scratch);
    for record in records {
        let mine = traced(record.callee);
        if mine.is_empty() {
            continue;
        }
        tuple.clear();
        for slot in mine {
            match record.at(*slot) {
                Some(Fixed::Content(content)) => tuple.push(content),
                _ => break,
            }
        }
        if tuple.len() == mine.len() {
            state.add(record.callee, &tuple, &edges);
        }
    }
    let mut next = 0;
    while let Some(&(caller, index)) = state.queue.get(next) {
        next += 1;
        if state.open[caller.index() as usize].is_some() {
            continue;
        }
        let binding = state.held[caller.index() as usize][index];
        let theirs = traced(caller);
        for record in out_of(caller).iter().map(|at| &records[*at]) {
            if !edge(record) || state.open[record.callee.index() as usize].is_some() {
                continue;
            }
            #[cfg(test)]
            EDGE_READS.with(|reads| reads.set(reads.get() + 1));
            tuple.clear();
            for slot in traced(record.callee) {
                match record.at(*slot) {
                    Some(Fixed::Content(content)) => tuple.push(content),
                    Some(Fixed::Passed(from)) => {
                        let position =
                            (theirs.binary_search(&from)).expect("a passed slot is traced");
                        tuple.push(binding[position]);
                    }
                    None => unreachable!("a body passed nothing at a traced slot is open"),
                }
            }
            state.add(record.callee, &tuple, &edges);
        }
    }

    let State { open, held, .. } = state;
    scratch.alloc_slice_fill_with(count, |at| {
        let slots = collect(scratch, slots[at].iter().copied());
        let bindings = match open[at] {
            Some(reason) => Bindings::Open(reason),
            None => {
                let mut sorted = BumpVec::with_capacity_in(held[at].len(), scratch);
                sorted.extend_from_slice(&held[at]);
                sorted.sort_unstable();
                Bindings::Named(collect(scratch, sorted.iter().copied()))
            }
        };
        Body { slots, bindings }
    })
}

/// The load's folds of every body `load` holds records for, its call sites traced, laid through
/// `lay` once: [`fold`](super::fold) staged in `scratch`, then [`trace`].
pub fn traced<'o>(
    load: &LoadFacts<'o>,
    lay: impl Lays<'o>,
    scratch: BumpAllocator<'_>,
) -> &'o [Folds<'o>] {
    trace(
        load,
        &super::folds::staged(load, lay, scratch),
        lay,
        scratch,
    )
}

/// `folds`, the keys fold's for `load`'s bodies, with each body keyed on its traced slots and each
/// shape's call sites of keyed bodies traced: [`bindings`] at the load's [`CAP`], then per site the
/// bindings it passes, the contents at its callee's keys, or why it names none. Laid through `lay`.
fn trace<'o>(
    load: &LoadFacts<'o>,
    folds: &[Folds<'o>],
    lay: impl Lays<'o>,
    scratch: BumpAllocator<'_>,
) -> &'o [Folds<'o>] {
    let shapes = load.shapes();
    let records = records(load, scratch);
    let bodies = enumerate(shapes, folds, &records, CAP, scratch);
    // A callable keys on its traced slots: its keys fold's and each slot it passes on.
    let keys_of = |callee: Segment| -> Option<&[Slot]> {
        let traced = bodies[callee.index() as usize].slots;
        match keys(shapes, folds, callee) {
            Some(_) => Some(traced),
            None if !traced.is_empty() && !shapes.shape_at(callee).deferred() => Some(traced),
            None => None,
        }
    };
    let mut traces: BumpVec<'_, (Segment, Site, CallTrace<'o>)> = BumpVec::new_in(scratch);
    // Each covered site's slot map, beside its owning body.
    let mut slot_maps: BumpVec<'_, (Segment, SlotMap<'o>)> = BumpVec::new_in(scratch);
    let mut tuple = BumpVec::new_in(scratch);
    for record in records.iter() {
        let Some(keys) = keys_of(record.callee) else {
            continue;
        };
        if !shapes.in_code(record.at) && keys.iter().all(|slot| record.at(*slot).is_some()) {
            let passed = (keys.iter()).map(|slot| (*slot, record.at(*slot).expect("covered")));
            slot_maps.push((
                shapes.owner(record.at),
                SlotMap {
                    at: record.at,
                    site: record.site,
                    call: record.call,
                    callee: record.callee,
                    passed: collect(lay, passed),
                },
            ));
        }
        let fixed = |slot: &Slot| record.at(*slot);
        let passes_on = keys
            .iter()
            .any(|slot| matches!(fixed(slot), Some(Fixed::Passed(_))));
        let caller = record.caller.map(|caller| bodies[caller.index() as usize]);
        let trace = if shapes.in_code(record.at) {
            CallTrace::Untraced(Untraced::Composed)
        } else if keys.iter().any(|slot| fixed(slot).is_none()) {
            CallTrace::Untraced(Untraced::Unfollowed)
        } else if let (
            true,
            Some(Body {
                bindings: Bindings::Open(reason),
                ..
            }),
        ) = (passes_on, caller)
        {
            CallTrace::Untraced(Untraced::Caller(reason))
        } else {
            // One binding per binding of the caller, or one alone where every key is content.
            let from: &[&[Content]] = match (passes_on, caller) {
                (
                    true,
                    Some(Body {
                        bindings: Bindings::Named(named),
                        ..
                    }),
                ) => named,
                _ => &[&[]],
            };
            let theirs = caller.map_or(&[][..], |caller| caller.slots);
            let mut bindings = BumpVec::with_capacity_in(from.len(), scratch);
            for binding in from {
                tuple.clear();
                for slot in keys {
                    tuple.push(match fixed(slot) {
                        Some(Fixed::Content(content)) => content,
                        Some(Fixed::Passed(from)) => {
                            binding[theirs
                                .binary_search(&from)
                                .expect("a passed slot is traced")]
                        }
                        None => unreachable!("every key is passed"),
                    });
                }
                bindings.push(collect(lay, tuple.iter().copied()));
            }
            bindings.sort_unstable();
            bindings.dedup();
            CallTrace::Named {
                callee: record.callee,
                bindings: collect(lay, bindings.iter().copied()),
            }
        };
        traces.push((record.at, record.site, trace));
    }
    // A site the text lists that no record names: the load resolves it to no one callee.
    for shape in shapes.bodies() {
        let callee = shape.segment();
        if keys_of(callee).is_none() {
            continue;
        }
        for call in shapes.calls_into(callee) {
            let recorded = records
                .binary_search_by_key(&(call.at, call.site), |record| (record.at, record.site))
                .is_ok();
            if call.offered || recorded {
                continue;
            }
            let reason = match shapes.in_code(call.at) {
                true => Untraced::Composed,
                false => Untraced::Unresolved,
            };
            traces.push((call.at, call.site, CallTrace::Untraced(reason)));
        }
    }
    traces.sort_by_key(|(at, site, _)| (*at, *site));
    traces.dedup_by_key(|(at, site, _)| (*at, *site));
    // By owner, then by shape and site within each owner.
    slot_maps.sort_by_key(|(owner, map)| (*owner, map.at, map.site));
    debug_assert!(
        slot_maps.iter().all(
            |(owner, map)| map.passed.iter().all(|(_, fixed)| match fixed {
                Fixed::Passed(slot) => keys_of(*owner).is_some_and(|keys| keys.contains(slot)),
                Fixed::Content(_) => true,
            })
        ),
        "a slot a site passes on is a key of its owner"
    );
    lay.fill(folds.len(), |at| {
        let segment = shapes.bodies()[at].segment();
        let from = traces.partition_point(|(each, ..)| *each < segment);
        let to = traces.partition_point(|(each, ..)| *each <= segment);
        let owned = {
            let from = slot_maps.partition_point(|(owner, _)| *owner < segment);
            let to = slot_maps.partition_point(|(owner, _)| *owner <= segment);
            collect(lay, slot_maps[from..to].iter().map(|(_, map)| *map))
        };
        Folds {
            keys: keys_of(segment).map(|keys| collect(lay, keys.iter().copied())),
            slot_maps: owned,
            traces: Keyed::from_sorted(collect(
                lay,
                traces[from..to]
                    .iter()
                    .map(|(_, site, trace)| (*site, *trace)),
            )),
            ..folds[at]
        }
    })
}
