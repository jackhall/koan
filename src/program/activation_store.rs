//! A frame's **activation store**: what a frame decides in its own activation at a binding event
//! whose names its depot slot's facts leave unbound
//! ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! The binding is read off the frame's slots: a type value binds its type, and a module value its
//! carried type over its bare member's content, by digest, beside that member as a view over it
//! reads it ([`Bare`](crate::scope::Bare)), under the header it is seen through, as a settlement
//! binds one. The walks are the language's, rooted at the frame's
//! body, over a layer whose base is everything the frame read below: the load, the settlements
//! between, and the events its activation decided before. An event walks closed, or as a
//! [resumption](super::resume) that witnesses its type names or reads over a witnessed layer whose
//! questions the binding keeps ([`Activating`]); a witnessing walk reads the witnessed and closed
//! layers beneath it alone, its cell's and its enclosers', its
//! [spine](crate::scope::Facts::spine), and witnesses the names their events witnessed too. It
//! walks at the step's borrow in a scratch bump;
//! the [records copier](crate::scope::BuildLayer::relaid) lays what it wrote in the frame's region
//! — a witnessed layer with what lies beneath bound alone, so it keeps the marks on the names it
//! witnessed — and it interns through the frame's own chain: the region's, or a caching frame's
//! scratch, whose exit adopts what the records name as it adopts what its values reach. No depot
//! byte is involved, and nothing reaches the agent tier. What a walk decided, the body runner
//! keeps in its frame's decision memo, so a later event over the same bindings, or over bindings
//! keeping a witnessed layer's questions, walks less.

use crate::build::LayerStore;
use std::cell::Cell;

use crate::knot::KValue;
use crate::knot::module::view;
use crate::memory::{Bump, BumpAllocator, BumpVec, Lays, Writer, resident};
use crate::scope::{
    Bare, Binding, Bound, BuildLayer, EventSite, Facts, Flattened, Keeping, RecordSource, Relay,
    Sealing, Segment, ShapeError, Tree, Unknown, relaid_error,
};
use crate::values::{Chain, ContentDigest, Value};

use super::record::{Language, Program};
use super::resume::{Resuming, resumed};

/// What a frame's slot holding `value` binds its unknown to: a type value its type, and a module
/// value its carried type over its bare member's content, by digest, beside that member as a view
/// over it reads it, retyped where a header carries it, as [`staged`](super::settle::staged)
/// binds a settlement's: a view over it reads the bare member, as the run's does. `None` for any
/// other value. `memo`, the slot's digest memo where the load marked it digested, holds the digest
/// of the value the slot holds: it is read for a bare member and filled where it is empty.
pub fn bound_of<'o>(
    lay: impl Lays<'o>,
    value: KValue<'_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    memo: Option<&Cell<Option<ContentDigest>>>,
) -> Option<Bound<'o>> {
    if let Some(named) = value.as_type() {
        return Some(Bound::Type(named.handle()));
    }
    value.as_module()?;
    let member = value.knot_member()?;
    let carried = value.concrete_ktype();
    // The record's carriers are laid beside the tree, which outlives the frame's slot.
    let bare = view::bare_of(member, types, scratch);
    let sealing = bare.sealing.map(|sealing| Sealing {
        record: sealing.record.laid(lay),
        ..sealing
    });
    let as_view = Some(resident(
        lay,
        Bare {
            signature: bare.signature,
            sealing,
        },
    ));
    let bare = Value::Knotted(member);
    // A header's digest is its type over its bare member's, which its memo cannot give back.
    let tree = match (bare.ktype() == value.ktype(), memo.and_then(Cell::get)) {
        (true, Some(digest)) => Tree::Digest(digest, as_view),
        (true, None) => Tree::Digest(bare.digest(types, scratch), as_view),
        (false, _) => {
            let member = resident(lay, Tree::Digest(bare.digest(types, scratch), as_view));
            Tree::retyped(carried, member)
        }
    };
    memo.filter(|memo| memo.get().is_none())
        .inspect(|memo| memo.set(tree.digest()));
    Some(Bound::Module {
        carried,
        tree: resident(lay, tree),
    })
}

#[cfg(test)]
thread_local! {
    /// How many binding events walked: what a test reads to see a frame decide, or not.
    pub static DECIDED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many decisions a memo kept: what a test reads to see a home stop growing.
    pub static KEPT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Whether every binding event walks, reading no memo: the law's oracle.
    pub static UNMEMOIZED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How a frame's binding event decides: walked closed, over a witnessed layer whose questions the
/// binding keeps (all or some), or witnessed afresh at `site` ([`resume`](super::resume)), the
/// names the witnessed layers beneath it witnessed beside its own.
#[derive(Clone, Copy)]
pub(crate) enum Activating<'h> {
    Closed,
    Over(&'h BuildLayer<'h>, Keeping),
    Witnessing { site: EventSite },
}

#[cfg(test)]
impl Activating<'_> {
    /// Whether deciding so walks: all but a binding keeping every question of a witnessed layer
    /// where nothing holds.
    pub(crate) fn walks(&self) -> bool {
        match self {
            Activating::Over(witnessed, Keeping::All) => witnessed
                .witnessing()
                .is_none_or(|witnessing| witnessing.holds),
            _ => true,
        }
    }
}

/// What a frame's binding event decided, laid in the frame's region: the witnessed layer its event
/// laid, where it witnessed, over the layers beneath; and the binding's layer, over that or over
/// the witnessed layer it read over.
pub(crate) struct Activated<'here> {
    pub witnessed: Option<&'here BuildLayer<'here>>,
    pub layer: BuildLayer<'here>,
}

/// The walk a frame's binding event decides through, the language's monomorphized in:
/// [`activated`].
pub(super) type Activate<'graph> =
    for<'here, 'w, 'a, 'bind> fn(
        &Program<'graph>,
        Writer<'here, 'w>,
        Facts<'a>,
        &Chain<'_, '_, '_, '_>,
        Option<&'here BuildLayer<'here>>,
        &[(Unknown, Bound<'bind>)],
        (Activating<'here>, bool),
        Segment,
    ) -> Result<Activated<'here>, &'here ShapeError<'here>>;

/// [`activated`] laying through a frame's region writer: what the program holds.
#[allow(clippy::too_many_arguments)]
pub(super) fn activated_in<'here, L: Language>(
    program: &Program<'_>,
    writer: Writer<'here, '_>,
    facts: Facts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    below: Option<&'here BuildLayer<'here>>,
    own: &[(Unknown, Bound<'_>)],
    activating: (Activating<'here>, bool),
    at: Segment,
) -> Result<Activated<'here>, &'here ShapeError<'here>> {
    activated::<L>(program, writer, facts, types, below, own, activating, at)
}

/// What the binding event binding `own` decides for the body `at` over `facts`, which read
/// `below`, its cell's and enclosers' earlier events, as `activating` says: `L`'s walks rooted at
/// the body, laid through `lay` and interned through `types`, the frame's chain. A closed walk
/// wakes the units whose records name what `own` binds, or, where it `rewakes`, any name `facts`
/// bind too. A refusal the walk reaches is laid through `lay`, for the cell to fault with as the
/// load would; its types lie in `types`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn activated<'here, L: Language>(
    program: &Program<'_>,
    lay: impl Lays<'here>,
    facts: Facts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    below: Option<&'here BuildLayer<'here>>,
    own: &[(Unknown, Bound<'_>)],
    (activating, rewakes): (Activating<'here>, bool),
    at: Segment,
) -> Result<Activated<'here>, &'here ShapeError<'here>> {
    #[cfg(test)]
    if activating.walks() {
        DECIDED.with(|decided| decided.set(decided.get() + 1));
    }
    let (bump, scratch) = (Bump::new(), Bump::new());
    // The walk reads its base's records whole, not through the typed readers: every type they
    // name resolves in the frame's chain before it runs.
    let base = facts.whole(types);
    let mut staged = BumpVec::new_in(&scratch);
    base.each_bound(&mut |unknown, bound| staged.push((unknown, bound)));
    // The names the witnessed layers beneath witnessed, its cell's and its enclosers', which a
    // witnessing walk witnesses again.
    let earlier = match activating {
        Activating::Witnessing { .. } => BuildLayer::witnessed_names(&scratch, below),
        _ => BumpVec::new_in(&scratch),
    };
    let mut alone = staged.clone();
    alone.retain(|(unknown, _)| earlier.binary_search(unknown).is_err());
    let beneath = Binding::laid(&bump, &mut alone);
    staged.extend(own.iter().copied());
    let binding = Binding::laid(&bump, &mut staged);
    let mut bound = BumpVec::new_in(&scratch);
    bound.extend(own.iter().copied());
    let mine = Binding::laid(&bump, &mut bound);
    let resuming = match activating {
        Activating::Closed => {
            let layer = match rewakes {
                true => LayerStore::waking_all(&bump, &scratch, base, binding),
                false => LayerStore::new(&bump, &scratch, base, binding),
            };
            // The base the frame's chain rests on: the load's shapes, or its composition's.
            let walked = L::facts(
                facts.shapes(),
                program.builtins(),
                &layer,
                types,
                &scratch,
                at,
            );
            super::resume::count_laid();
            return match walked {
                Ok(()) => Ok(Activated {
                    witnessed: None,
                    layer: BuildLayer::relaid(
                        lay,
                        &scratch,
                        facts.shapes(),
                        below,
                        binding,
                        own,
                        layer.writes(),
                    ),
                }),
                Err(error) => Err(relaid_error(lay, &scratch, &error)),
            };
        }
        Activating::Over(witnessed, kept) => Resuming::Over(witnessed, kept),
        Activating::Witnessing { site } => {
            let mut names = BumpVec::new_in(&scratch);
            names.extend(
                (mine.entries().iter()).filter(|(_, bound)| matches!(bound, Bound::Type(_))),
            );
            names.extend((earlier.iter()).filter_map(|name| Some((*name, binding.bound(*name)?))));
            Resuming::Witnessing {
                spine: facts.spine(&bump, &scratch, below),
                witness: Binding::laid(&bump, &mut names),
                site,
            }
        }
    };
    let mut flat = Flattened::new(&scratch);
    let resumed = resumed::<L>(
        (facts.shapes(), program.builtins()),
        (&bump, &scratch, &mut flat),
        facts,
        (binding, mine),
        (resuming, rewakes),
        types,
        at,
    )
    .map_err(|error| relaid_error(lay, &scratch, &error))?;
    // One copier lays both layers, over the frontiers the walks flattened: a witnessed layer keeps
    // the marks on the names it witnessed, relayed with what lies beneath bound alone, less the
    // names earlier events witnessed; the binding's layer drops every name the binding binds.
    let mut relay = Relay::flattening(lay, &scratch, beneath, flat);
    let witnessed = (resumed.witnessed)
        .map(|witnessed| resident(lay, witnessed.relayed(lay, &scratch, &mut relay, below)));
    let parent = match (witnessed, activating) {
        (Some(witnessed), _) => Some(witnessed),
        (None, Activating::Over(read, _)) => Some(resident(lay, read.over(below))),
        (None, _) => below,
    };
    relay.rebind(binding);
    Ok(Activated {
        witnessed,
        layer: resumed.layer.relayed(lay, &scratch, &mut relay, parent),
    })
}
