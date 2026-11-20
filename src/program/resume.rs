//! A **resumption**: how a binding event's layer is laid over a cell's chain
//! ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! A resumption walks **closed** — one walk binding everything — or over a
//! **witnessed layer** the binding keeps the questions of, or **witnesses** afresh. To witness, it
//! runs two walks over one rooted body. The first, over the spine its caller hands it, binds its
//! base's names and the event's module binders and leaves the witnessed names rigid, each door's
//! undecided question answered under the witness ([`LayerStore::witnessing`]); its records are the
//! witnessed layer. The second, a closing walk over the cell's chain with the witnessed layer
//! innermost, re-runs every unit whose frontier reaches a question the binding does not keep or a
//! unit the first walk held, and lays the binding's layer over the witnessed one. A first walk
//! that refuses is dropped and the event walked closed, so a closed walk words every refusal.
//!
//! Over a witnessed layer whose questions a later binding keeps, and where nothing holds — the
//! first closing walk re-ran no unit, substituted no open content tree and closed no key reading
//! a module binder the witness binds otherwise than the binding's levels do, and no witnessed
//! closing reached a question it did not keep — no walk runs: the binding's layer is its table of
//! closed types alone, closed over the keys the first closing walk laid and those the witnessed
//! closings name ([`closed_under`]), which it reads through. Otherwise the closing walk runs alone
//! over it.
//!
//! Every walk lays through the one `lay` its caller hands in: program storage for the load, a
//! depot slot's bump for a settlement, a scratch bump an activation relays into its region.

use crate::build::LayerStore;
use crate::memory::{Bump, BumpVec, Lays, resident};
use crate::scope::{
    Admission, Answered, Binding, Bound, BuildLayer, Derived, DerivedName, EventSite, Facts,
    Flattened, Frontier, Keeping, KeyedWrites, Levels, LevelsOver, Mark, Segment, ShapeError,
    Shapes, Spellings, Store, Unknown, Wake, Witnessing, closed_under, keeps, rebound,
    witnessed_digest,
};
use crate::values::Chain;

use super::record::Language;

#[cfg(test)]
thread_local! {
    /// Test-only: walk every resumption closed — the report's before column.
    pub(crate) static UNWITNESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// How many binding layers a resumption laid, walked or not. Tests reset it.
    pub(crate) static LAYERS_LAID: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many witnessing walks ran. Tests reset it.
    pub(crate) static WITNESS_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether a resumption may witness: not where a test walks every one closed.
pub(crate) fn witnesses() -> bool {
    #[cfg(test)]
    if UNWITNESSED.with(std::cell::Cell::get) {
        return false;
    }
    true
}

/// How a resumption decides.
#[derive(Clone, Copy)]
pub(crate) enum Resuming<'a> {
    /// One closing walk binding everything.
    Closed,
    /// Over a witnessed layer, re-parented over the cell's innermost, whose questions the binding
    /// keeps as many of as it says.
    Over(&'a BuildLayer<'a>, Keeping),
    /// Witnessed afresh at `site`: a first walk over `spine` leaving `witness` rigid, then a
    /// closing walk over the cell's chain.
    Witnessing {
        spine: Facts<'a>,
        witness: Binding<'a>,
        site: EventSite,
    },
}

/// What a resumption laid: the witnessed layer, where it witnessed afresh, and the binding's
/// layer, over it where there is one.
pub(crate) struct Resumed<'a> {
    pub(crate) witnessed: Option<&'a BuildLayer<'a>>,
    pub(crate) layer: BuildLayer<'a>,
}

/// The layer the event at `body` binding `own` lays over `facts`, the cell's chain, whose names
/// with the event's are `binding`, as `resuming` says: walked through `L` and laid through `lay`,
/// interned through `types`. A closed walk wakes every name `facts` bind too where it `rewakes`. A
/// witnessing walk gathers its admission through `flat`, which a caller relaying the layers
/// adopts, so each frontier is flattened once. A refusal is a closing walk's.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resumed<'a, 'graph: 'a, 'tier: 'a, L: Language>(
    (shapes, builtins, spellings): (
        &'graph Shapes<'graph>,
        &'graph crate::knot::KBuiltins<'graph, 'graph>,
        Spellings<'_>,
    ),
    (lay, scratch, flat): (impl Lays<'a>, &Bump, &mut Flattened<'_, 'a>),
    facts: Facts<'a>,
    (binding, own): (Binding<'a>, Binding<'a>),
    (resuming, rewakes): (Resuming<'a>, bool),
    types: &Chain<'tier, '_, '_, '_>,
    body: Segment,
) -> Result<Resumed<'a>, ShapeError<'a>> {
    let top = facts.innermost();
    match resuming {
        Resuming::Closed => {
            let base = facts.whole(types);
            let stores = (lay, scratch, spellings);
            let store = match rewakes {
                true => LayerStore::waking_all(stores, base, binding),
                false => LayerStore::new(stores, base, binding),
            };
            L::facts(shapes, builtins, &store, types, scratch, body)?;
            count_laid();
            Ok(Resumed {
                witnessed: None,
                layer: BuildLayer::of(lay, scratch, shapes, top, own, store.writes()),
            })
        }
        Resuming::Over(witnessed, kept) => {
            let over = witnessed.resting_over(lay, top);
            let witnessing = witnessed
                .witnessing()
                .expect("a resumption reads over a witnessed layer");
            let types_of = type_names(lay, scratch, own);
            if kept == Keeping::All && !witnessing.holds {
                let levels = Levels::of(scratch, shapes, binding);
                let beneath = facts.beneath(over);
                let laid = std::cell::RefCell::new(BumpVec::new_in(scratch));
                let binds = LevelsOver {
                    levels: &levels,
                    names: &|id| beneath.derived_name(id),
                    spellings,
                    laid: &|name: DerivedName<'_>| laid.borrow_mut().push(name.laid(scratch)),
                };
                let closed = closed_under(lay, scratch, witnessing.keys, &mut |key| {
                    rebound(types, scratch, key, &binds)
                });
                let derived = Derived::sorted(lay, scratch, &laid.borrow());
                count_laid();
                let admission = Admission::binding(lay, own);
                return Ok(Resumed {
                    witnessed: None,
                    layer: BuildLayer::kept(over, types_of, admission, (closed, derived)),
                });
            }
            let checked = checked(lay, scratch, binding, witnessing.names);
            let (writes, _) = closing::<L>(
                (shapes, builtins, spellings),
                (lay, scratch),
                facts.beneath(over),
                (binding, checked),
                (types, &[]),
                body,
            )?;
            count_laid();
            Ok(Resumed {
                witnessed: None,
                layer: BuildLayer::of(lay, scratch, shapes, Some(over), types_of, &writes),
            })
        }
        Resuming::Witnessing {
            spine,
            witness,
            site,
        } => {
            #[cfg(test)]
            WITNESS_WALKS.with(|walks| walks.set(walks.get() + 1));
            let base = spine.whole(types);
            let store = LayerStore::witnessing((lay, scratch, spellings), base, own, witness);
            if L::facts(shapes, builtins, &store, types, scratch, body).is_err() {
                // A closed walk words every refusal.
                return resumed::<L>(
                    (shapes, builtins, spellings),
                    (lay, scratch, flat),
                    facts,
                    (binding, own),
                    (Resuming::Closed, rewakes),
                    types,
                    body,
                );
            }
            let modules = module_binders(lay, scratch, own);
            let laid = BuildLayer::of(lay, scratch, shapes, top, modules, store.writes());
            let provisional = resident(lay, laid);
            let names = witness.entries().iter().map(|(unknown, _)| *unknown);
            let names = crate::memory::collect(lay, names);
            let checked = checked(lay, scratch, binding, names);
            let (_, closings) = store.writes().sorted(scratch);
            let mut held = BumpVec::new_in(scratch);
            held.extend(closings.iter().map(|(_, (_, frontier))| *frontier));
            let (writes, rewrote) = closing::<L>(
                (shapes, builtins, spellings),
                (lay, scratch),
                facts.beneath(provisional),
                (binding, checked),
                (types, &held),
                body,
            )?;
            let answered = answered_on((scratch, flat), store.writes(), witness);
            // A binding laying no walk reads the witnessed closings through its table, which the
            // closing walk, rewriting them, never named.
            let mut keys = BumpVec::new_in(scratch);
            keys.extend(writes.closed().keys().iter().copied());
            for (_, (closing, _)) in closings.iter() {
                closing.each_type(&mut |ty| keys.push(ty));
            }
            let admission = Admission::keeping(lay, &answered);
            // The witness's module binders are its layer's, so every binding joining it binds
            // them alike: a key the closing walk closed as the levels of the binding's chain do
            // needs no walk. A key the walk never named walks where it reads a module binder.
            let beneath = facts.beneath(provisional);
            let levels = Levels::of(scratch, shapes, binding);
            let by_levels = LevelsOver {
                levels: &levels,
                names: &|id| beneath.derived_name(id),
                spellings,
                laid: &|_: DerivedName<'_>| {},
            };
            let walked = writes.closed();
            let over_modules = !modules.entries().is_empty()
                && (keys.iter()).any(|key| match walked.probe(*key) {
                    Some(made) => made != rebound(types, scratch, *key, &by_levels),
                    None => crate::scope::reads_module_binders(types, scratch, *key, &|id| {
                        beneath.derived_name(id)
                    }),
                });
            let witnessing = Witnessing {
                names,
                keys: crate::memory::collect(lay, keys.iter().copied()),
                // A binding keeping every question closes the keys by the levels of its chain,
                // which substitute no tree and close no module binder's class or carried type: a
                // layer whose closing walk substitutes an open tree, or whose keys read a module
                // binder its witness binds, walks.
                holds: rewrote || walked.substitutes_trees() || over_modules,
                digest: witnessed_digest(site, names, modules, admission),
            };
            let witnessed = resident(lay, laid.witnessed(admission, witnessing));
            count_laid();
            let layer = BuildLayer::of(
                lay,
                scratch,
                shapes,
                Some(witnessed),
                type_names(lay, scratch, own),
                &writes,
            );
            Ok(Resumed {
                witnessed: Some(witnessed),
                layer,
            })
        }
    }
}

/// A closing walk over `facts` binding `binding`, checking `checked`: its writes, beside whether
/// it holds anything — re-ran a unit, or reached a question it does not keep from one of the
/// closings' frontiers `witnessed`. Walking a body rewrites its closing whether or not it wakes.
fn closing<'a, 'p, 'graph: 'a, 'tier: 'a, L: Language>(
    (shapes, builtins, spellings): (
        &'graph Shapes<'graph>,
        &'graph crate::knot::KBuiltins<'graph, 'graph>,
        Spellings<'p>,
    ),
    (lay, scratch): (impl Lays<'a>, &'p Bump),
    facts: Facts<'a>,
    (binding, checked): (Binding<'a>, Binding<'a>),
    (types, witnessed): (&Chain<'tier, '_, '_, '_>, &[Frontier<'a>]),
    body: Segment,
) -> Result<(KeyedWrites<'a, 'p>, bool), ShapeError<'a>> {
    let store = LayerStore::over_witnessed(
        (lay, scratch, spellings),
        facts.whole(types),
        binding,
        checked,
    );
    L::facts(shapes, builtins, &store, types, scratch, body)?;
    let woke = (witnessed.iter()).any(|frontier| store.wakes(*frontier, types) == Wake::Open);
    let writes = store.into_writes();
    let holds = woke || writes.wrote_units();
    Ok((writes, holds))
}

/// What `binding` gives each of `names`, laid through `lay`: the names a closing walk over a
/// witnessed layer checks.
fn checked<'a>(
    lay: impl Lays<'a>,
    scratch: &Bump,
    binding: Binding<'a>,
    names: &[Unknown],
) -> Binding<'a> {
    let mut staged = BumpVec::new_in(scratch);
    staged.extend((names.iter()).filter_map(|unknown| Some((*unknown, binding.bound(*unknown)?))));
    Binding::laid(lay, &mut staged)
}

/// The module binders `own` binds, laid through `lay`: what a witnessed layer binds.
fn module_binders<'a>(lay: impl Lays<'a>, scratch: &Bump, own: Binding<'a>) -> Binding<'a> {
    let mut staged = BumpVec::new_in(scratch);
    staged
        .extend((own.entries().iter()).filter(|(_, bound)| matches!(bound, Bound::Module { .. })));
    Binding::laid(lay, &mut staged)
}

/// The type names `own` binds, laid through `lay`: what a binding's layer over a witnessed one
/// binds.
fn type_names<'a>(lay: impl Lays<'a>, scratch: &Bump, own: Binding<'a>) -> Binding<'a> {
    let mut staged = BumpVec::new_in(scratch);
    staged.extend((own.entries().iter()).filter(|(_, bound)| matches!(bound, Bound::Type(_))));
    Binding::laid(lay, &mut staged)
}

/// Every answered question on a witnessed name that the frontiers a witnessing walk laid reach:
/// their own marks and, through their reads, each read's past its cut — the encloser's records'
/// included — since a record built over an answer it read is reused wherever the admission is
/// kept.
fn answered_on<'a, 'x>(
    (scratch, flat): (&'x Bump, &mut Flattened<'_, 'a>),
    writes: &KeyedWrites<'a, '_>,
    witness: Binding<'_>,
) -> BumpVec<'x, &'a Answered<'a>> {
    let mut answered = BumpVec::new_in(scratch);
    let (units, closings) = writes.sorted(scratch);
    let mut each = |frontier: Frontier<'a>| {
        for noted in flat.of(frontier) {
            if let Mark::Open(open) = noted.mark
                && let Some(question) = open.answered
                && witness.bound(noted.unknown).is_some()
            {
                answered.push(question);
            }
        }
    };
    for (facts, frontier) in units.iter().filter_map(|(_, record)| record.typed()) {
        each(frontier);
        (facts.seeds.iter()).for_each(|(_, seed)| each(*seed));
    }
    closings
        .iter()
        .for_each(|(_, (_, frontier))| each(*frontier));
    answered
}

/// Whether a binding whose levels are `levels` keeps a candidate's `answered`, judged through
/// `types`: each derived name its question holds read off the candidate's layers first, which laid
/// it, then `facts`', and a term no table holds named through `spellings`.
pub(crate) fn kept_by<'q>(
    (types, scratch, spellings): (&'q Chain<'_, '_, '_, '_>, &'q Bump, Spellings<'q>),
    facts: Facts<'q>,
    levels: &'q Levels<'_>,
) -> impl Fn(&BuildLayer<'_>, &Answered<'_>) -> bool + 'q {
    move |candidate, answered| {
        let names = |id| {
            candidate
                .derived_name(id)
                .or_else(|| facts.derived_name(id))
        };
        let binds = LevelsOver {
            levels,
            names: &names,
            spellings,
            laid: &|_| {},
        };
        keeps(types, scratch, answered, &binds)
    }
}

/// Count a binding layer laid, walked or not.
pub(crate) fn count_laid() {
    #[cfg(test)]
    LAYERS_LAID.with(|laid| laid.set(laid.get() + 1));
}

/// The candidate whose witnessed layer a binding binding `own` reads over, beside how many of its
/// questions it keeps: the first that keeps every question, else the one keeping most. `None`
/// where none keeps any. A witnessed layer binds its event's module binders, so a binding of
/// others is no fit.
pub(crate) fn chosen<'a, T>(
    candidates: impl Iterator<Item = (T, &'a BuildLayer<'a>)>,
    keeps: &dyn Fn(&BuildLayer<'_>, &Answered<'_>) -> bool,
    own: Binding<'_>,
) -> Option<(T, Keeping)> {
    let mut best: Option<(T, usize)> = None;
    for (carried, candidate) in candidates {
        let modules_alike = (candidate.binding().entries().iter())
            .all(|(unknown, bound)| own.bound(*unknown).is_some_and(|mine| mine.same_as(bound)));
        if !modules_alike {
            continue;
        }
        match candidate
            .admission()
            .kept(&mut |answered| keeps(candidate, answered))
        {
            Keeping::All => return Some((carried, Keeping::All)),
            Keeping::Some(count) if best.as_ref().is_none_or(|(_, held)| count > *held) => {
                best = Some((carried, count));
            }
            Keeping::Some(_) | Keeping::None => {}
        }
    }
    best.map(|(carried, count)| (carried, Keeping::Some(count)))
}
