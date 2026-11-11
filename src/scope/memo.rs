//! The **decision memo**: what the cells of a tail chain decided in their activations, kept in the
//! region their results are built in so a later cell running the same body over the same bindings
//! reuses it instead of walking ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! A memo holds an [`Entry`] per key, keyed by the body's segment, the depot slot its cell runs
//! under, and the [digest](crate::scope::spine_digest) of the **spine** beneath the cell's first event: each
//! witnessed layer there by its event's site, the names it witnessed, the module binders it binds
//! and its admission, and each layer walked closed by every name it was walked under. So a block,
//! a module body or a callee whose closure carries its encloser's layers finds one entry under
//! every encloser binding that keeps the encloser's questions. An entry holds a **tree** of
//! [`Decision`]s: each sits under the decision its walk was laid over at the event before, at most
//! [`FANOUT`] under one parent, the earliest laid dropped first. A decision holds the **witnessed
//! layer** its event laid, where it witnessed, and up to [`FANOUT`] **binding layers** over it,
//! one per binding it decided; a closed decision holds one, over the layers beneath. A cell walks
//! down the tree event by event, finding at each a decision whose site is the event's holding a
//! binding layer whose [admission](super::Admission) the event's binding meets by identity of
//! every name its chain binds; it holds only the last decision it reached, since a decision lies
//! once in its entry's tree and a miss finds the path down to it there. Where none admits, its
//! caller asks the witnessed decisions at the event ([`Entry::witnessed`]) whether the binding
//! keeps their questions. Keeping every one, the binding's layer joins the decision it reads over
//! ([`Decided::Joins`]); keeping some, it witnesses afresh ([`Decided::New`]) while the event has
//! room for another decision, else joins the one keeping most. So a loop alternating two bindings
//! at one event decides each once, and bindings of novel types share one witnessed layer's
//! records per answer their questions take. Every event witnesses the names the spine's witnessed
//! layers witnessed — its cell's earlier events' and its enclosers' — beside its own, so its
//! witnessed decision serves every binding beneath keeping its questions.
//!
//! A memo is immutable once laid: a persistent hash trie of its entries by key, so a miss lays a
//! new memo copying only the trie's path to its key and, within the entry, the path from the root
//! to the new decision, sharing every sibling along both. A lookup or a lay costs the log of the
//! distinct keys along the homes a memo was seeded through, however many lays they made, so a
//! non-tail recursion pays no more per level as it deepens. What a half lays is bounded by its
//! [budget](super::HalfHeader::memo_room). Where a memo rests, and how a cell reaches it, is
//! `program`'s: its region header's [`memo`](super::HalfHeader::memo).

#[cfg(test)]
use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::memory::{Bump, BumpVec, DropFree, Lays, collect, covariant, reattachable, resident};
use crate::type_lattice::{DeclaredType, Parametric};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{ContentDigest, DepotRef};

use super::admission::Admission;
use super::build_layer::BuildLayer;
use super::facts::{Binding, Bound, Unknown};
use super::relay::Relay;
use super::shape::Segment;
use super::tree::Identity;

/// How many decisions an entry keeps at one event under one parent.
pub const FANOUT: usize = 4;

/// Where a binding event fires: a body's entry, or the statements it ran up to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EventSite {
    segment: Segment,
    at: u32,
}

impl EventSite {
    /// The entry of the body `segment`.
    pub fn entry(segment: Segment) -> Self {
        EventSite {
            segment,
            at: u32::MAX,
        }
    }

    /// The seam after the body `segment` ran its statements up to `ran`.
    pub fn seam(segment: Segment, ran: u32) -> Self {
        EventSite { segment, at: ran }
    }

    /// The body the event fires in.
    pub fn segment(self) -> Segment {
        self.segment
    }
}

/// What an entry is found by: the body, the depot slot its cell runs under, and the
/// [digest](crate::scope::spine_digest) of the spine beneath the cell's first event.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoKey {
    pub segment: Segment,
    pub depot: DepotRef,
    pub spine: ContentDigest,
}

impl MemoKey {
    /// The key of a cell running `segment` under `depot` whose first event lies over `below`.
    pub fn new(segment: Segment, depot: DepotRef, below: Option<&BuildLayer<'_>>) -> Self {
        MemoKey {
            segment,
            depot,
            spine: spine_digest(below),
        }
    }

    /// The hash a memo's trie branches on.
    fn hashed(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        let hash = hasher.finish();
        #[cfg(test)]
        let hash = hash & HASH_MASK.with(Cell::get);
        hash
    }
}

/// The family of a region's memo, as a door reads it out of the region's header.
pub struct MemoFamily;
reattachable!(MemoFamily => Option<&'cell DecisionMemo<'cell>>);
// A memo is pinned at a reader's shorter brand.
covariant!(MemoFamily);
impl DropFree for MemoFamily {}

/// The family of whether a region's half may lay another memo entry, as a door reads it.
pub struct RoomFamily;
reattachable!(RoomFamily => bool);
covariant!(RoomFamily);
impl DropFree for RoomFamily {}

/// The family of the entry a miss laid in a region's memo.
pub struct EntryFamily;
reattachable!(EntryFamily => Option<&'cell Entry<'cell>>);
covariant!(EntryFamily);
impl DropFree for EntryFamily {}

#[cfg(test)]
thread_local! {
    /// What a key's hash is masked by: a test clears bits so keys collide.
    pub(crate) static HASH_MASK: Cell<u64> = const { Cell::new(u64::MAX) };
    /// The most trie nodes one lookup visited: what a test reads to see lookups stay shallow.
    pub(crate) static PROBED: Cell<usize> = const { Cell::new(0) };
}

/// How many bits of a key's hash one level of a memo's trie branches on.
const BITS: u32 = 5;

/// One memo: a node of a persistent hash trie of its entries, branching 32 ways on the bits of
/// each key's hash a level at a time, so a lookup or a lay costs the log of the distinct keys
/// along its homes, however many lays they made.
pub struct DecisionMemo<'a> {
    /// Which of the 32 branches hold a child, a bit each.
    bitmap: u32,
    /// The children, in branch order.
    children: &'a [Child<'a>],
}

/// What one branch of a memo's trie holds.
#[derive(Clone, Copy)]
enum Child<'a> {
    Entry(&'a Entry<'a>),
    /// The node one level down.
    Node(&'a DecisionMemo<'a>),
    /// The entries whose keys hash alike to the last bit.
    Collided(&'a [&'a Entry<'a>]),
}

/// What the cells running one body over one key decided, event by event, as a tree.
pub struct Entry<'a> {
    key: MemoKey,
    /// The decisions at the first event.
    roots: &'a [&'a Decision<'a>],
    /// The decision whose miss laid this entry, where the laying cell's walk down it now rests.
    laid: Option<&'a Decision<'a>>,
}

/// One binding event's decision: where it fired, the witnessed layer its event laid, each
/// binding's layer, and the decisions at the next event, walked over any of them. Every layer
/// rests alone, its parent set where a cell reads it ([`over`](Decision::over)).
pub struct Decision<'a> {
    site: EventSite,
    /// The witnessed layer laid at this event: `None` for a decision walked closed.
    witnessed: Option<&'a BuildLayer<'a>>,
    /// Each binding's layer, which binds the names it bound alone, admitting by identity: at most
    /// [`FANOUT`], the earliest dropped. A closed decision holds one.
    bindings: &'a [BuildLayer<'a>],
    next: &'a [&'a Decision<'a>],
}

/// What a binding event decided, for [`extended`] to keep: a decision of its own, or a binding's
/// layer joining a witnessed decision whose questions it keeps.
#[derive(Clone, Copy)]
pub enum Decided<'d> {
    /// A new decision at the event: its witnessed layer where it witnessed, beside its binding's.
    New {
        witnessed: Option<&'d BuildLayer<'d>>,
        binding: &'d BuildLayer<'d>,
    },
    /// The binding's layer of an event that read over `into`'s witnessed layer, a decision under
    /// the cell's path.
    Joins {
        into: &'d Decision<'d>,
        binding: &'d BuildLayer<'d>,
    },
}

impl<'a> DecisionMemo<'a> {
    /// The entry `key` finds.
    pub fn find(&self, key: MemoKey) -> Option<&'a Entry<'a>> {
        let hash = key.hashed();
        let mut node = self;
        let mut shift = 0;
        loop {
            #[cfg(test)]
            PROBED.with(|probed| probed.set(probed.get().max((shift / BITS) as usize + 1)));
            let bit = 1u32 << ((hash >> shift) & 31);
            if node.bitmap & bit == 0 {
                return None;
            }
            match node.children[(node.bitmap & (bit - 1)).count_ones() as usize] {
                Child::Entry(entry) => return (entry.key == key).then_some(entry),
                Child::Node(below) => (node, shift) = (below, shift + BITS),
                Child::Collided(entries) => {
                    return entries.iter().copied().find(|entry| entry.key == key);
                }
            }
        }
    }
}

/// `node`, the trie of a memo at the level `shift` bits down, or an empty one, with `entry` in
/// place of the entry of its key, `hash` its key's: laid through `lay`, the path to it alone laid
/// again and every other branch shared.
fn with_entry<'to, L: Lays<'to>>(
    lay: L,
    node: Option<&'to DecisionMemo<'to>>,
    entry: &'to Entry<'to>,
    hash: u64,
    shift: u32,
) -> &'to DecisionMemo<'to> {
    let (bitmap, children) = node.map_or((0, &[][..]), |node| (node.bitmap, node.children));
    let bit = 1u32 << ((hash >> shift) & 31);
    let at = (bitmap & (bit - 1)).count_ones() as usize;
    let held = (bitmap & bit != 0).then(|| children[at]);
    let child = match held {
        None => Child::Entry(entry),
        Some(Child::Entry(held)) if held.key == entry.key => Child::Entry(entry),
        // Past the hash's last bit, keys alike in every bit lie side by side.
        Some(Child::Entry(held)) if shift + BITS >= u64::BITS => {
            Child::Collided(lay.fill(2, |at| [held, entry][at]))
        }
        Some(Child::Entry(held)) => {
            let below = with_entry(lay, None, held, held.key.hashed(), shift + BITS);
            Child::Node(with_entry(lay, Some(below), entry, hash, shift + BITS))
        }
        Some(Child::Node(below)) => {
            Child::Node(with_entry(lay, Some(below), entry, hash, shift + BITS))
        }
        Some(Child::Collided(held)) => {
            let scratch = Bump::new();
            let mut kept = BumpVec::new_in(&scratch);
            kept.extend(held.iter().copied().filter(|each| each.key != entry.key));
            kept.push(entry);
            Child::Collided(collect(lay, kept.iter().copied()))
        }
    };
    let children = match held {
        Some(_) => lay.fill(children.len(), |each| match each == at {
            true => child,
            false => children[each],
        }),
        None => lay.fill(children.len() + 1, |each| match each.cmp(&at) {
            std::cmp::Ordering::Less => children[each],
            std::cmp::Ordering::Equal => child,
            std::cmp::Ordering::Greater => children[each - 1],
        }),
    };
    resident(
        lay,
        DecisionMemo {
            bitmap: bitmap | bit,
            children,
        },
    )
}

impl<'a> Entry<'a> {
    /// The decision under `at`, the entry's first event where `None`, holding a binding layer an
    /// event at `site` binding `own` over `below` is admitted to, beside that layer.
    pub fn decision(
        &self,
        at: Option<&Decision<'a>>,
        site: EventSite,
        own: &[(Unknown, Bound<'_>)],
        below: Option<&BuildLayer<'_>>,
    ) -> Option<(&'a Decision<'a>, &'a BuildLayer<'a>)> {
        let held = at.map_or(self.roots, |at| at.next);
        (held.iter().copied())
            .filter(|decision| decision.site == site)
            .find_map(|decision| {
                let binding = (decision.bindings.iter())
                    .find(|binding| binding.admission().admits(own, below))?;
                Some((decision, binding))
            })
    }

    /// The witnessed decisions under `at` at `site` whose witnessed layer an event binding `own`
    /// may read over: it witnessed the type names `own` binds and `earlier`, those the witnessed
    /// layers beneath witnessed, its cell's earlier events' and its enclosers', and bound its
    /// module binders alike. Whether the binding keeps its questions is its caller's to ask.
    pub fn witnessed<'q>(
        &'q self,
        at: Option<&'q Decision<'a>>,
        site: EventSite,
        own: &'q [(Unknown, Bound<'_>)],
        earlier: &'q [Unknown],
    ) -> impl Iterator<Item = &'a Decision<'a>> + 'q {
        let held = at.map_or(self.roots, |at| at.next);
        (held.iter().copied()).filter(move |decision| {
            decision.site == site
                && decision
                    .witnessed
                    .is_some_and(|witnessed| witnesses(witnessed, own, earlier))
        })
    }

    /// Whether another decision fits under `at`, the entry's first event where `None`, with none
    /// dropped: fewer than [`FANOUT`] are held there.
    pub fn room_under(&self, at: Option<&Decision<'a>>) -> bool {
        at.map_or(self.roots, |at| at.next).len() < FANOUT
    }

    /// The decision whose miss laid this entry.
    pub fn laid(&self) -> Option<&'a Decision<'a>> {
        self.laid
    }

    /// Hand `each` every type the entry's decisions name, the whole tree.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        each_decision(self.roots, &mut |decision| decision.each_type(each));
    }
}

impl<'a> Decision<'a> {
    /// The witnessed layer laid at this event, where it witnessed.
    pub fn witnessed(&self) -> Option<&'a BuildLayer<'a>> {
        self.witnessed
    }

    /// `binding`, one of this decision's, as its cell's layer over `below`: over the witnessed
    /// layer, laid over `below` through `lay` where it does not already lie there, where the
    /// decision witnessed.
    pub fn over(
        &self,
        lay: impl Lays<'a>,
        binding: &BuildLayer<'a>,
        below: Option<&'a BuildLayer<'a>>,
    ) -> BuildLayer<'a> {
        let parent = match self.witnessed {
            Some(witnessed) => Some(witnessed.resting_over(lay, below)),
            None => below,
        };
        binding.over(parent)
    }

    /// Hand `each` every type its layers name.
    fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for layer in self.witnessed.into_iter().chain(self.bindings.iter()) {
            layer.each_own_type(each);
            each_bound_type(layer.binding().entries(), each);
        }
    }
}

/// Whether an event binding `own` may read over `witnessed`: it witnessed exactly the type names
/// `own` binds and `earlier`, and binds each module binder `own` binds alike, and no other.
fn witnesses(
    witnessed: &BuildLayer<'_>,
    own: &[(Unknown, Bound<'_>)],
    earlier: &[Unknown],
) -> bool {
    let Some(witnessing) = witnessed.witnessing() else {
        return false;
    };
    let modules = witnessed.binding();
    let (mut names, mut binders) = (0, 0);
    let each = own.iter().all(|(unknown, bound)| match bound {
        Bound::Type(_) => {
            names += 1;
            witnessing.names.contains(unknown)
        }
        Bound::Module { .. } => {
            binders += 1;
            modules
                .bound(*unknown)
                .is_some_and(|held| held.same_as(bound))
        }
    });
    let earlier_witnessed = (earlier.iter()).all(|name| witnessing.names.contains(name));
    each && earlier_witnessed
        && names + earlier.len() == witnessing.names.len()
        && binders == modules.entries().len()
}

/// The digest of a witnessed layer laid at `site`, which witnessed `names`, binds `binders`, its
/// event's module binders, and admits by `admission`: what the spine of a cell over it carries.
pub fn witnessed_digest(
    site: EventSite,
    names: &[Unknown],
    binders: Binding<'_>,
    admission: Admission<'_>,
) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Witnessed);
    hasher.feed(site).count(names.len());
    names.iter().for_each(|name| _ = hasher.feed(name));
    feed_binding(&mut hasher, binders.entries());
    let mut kept = 0;
    admission.each_kept(&mut |_| kept += 1);
    hasher.count(kept);
    admission.each_kept(&mut |answered| _ = hasher.digest(answered.digest()));
    hasher.finished()
}

/// The digest of the spine beneath a cell's first event, `below` its top: each witnessed layer by
/// its [`witnessed_digest`], and each layer walked closed by every name it was walked under, its
/// own and those of every layer beneath it, innermost first. A binding's layer over a witnessed
/// one is left out: the witnessed layer's admission stands for what its bindings keep.
pub fn spine_digest(below: Option<&BuildLayer<'_>>) -> ContentDigest {
    let mut hasher = DigestHasher::new(Tag::Spine);
    let spine = || {
        std::iter::successors(below, |layer| layer.parent()).filter(|layer| !layer.over_witnessed())
    };
    hasher.count(spine().count());
    for layer in spine() {
        let digest = match layer.witnessing() {
            Some(witnessing) => {
                // An activation binds a module by its content's digest, which names it.
                debug_assert!(
                    (layer.binding().entries().iter()).all(|(_, bound)| match bound {
                        Bound::Module { tree, .. } => tree.identity() != Identity::Unnamed,
                        Bound::Type(_) => true,
                    }),
                    "a witnessed layer under an activation binds named modules"
                );
                witnessing.digest
            }
            None => {
                let mut closed = DigestHasher::new(Tag::Binding);
                for walked in std::iter::successors(Some(layer), |layer| layer.parent()) {
                    feed_binding(&mut closed, walked.binding().entries());
                }
                closed.finished()
            }
        };
        hasher.digest(digest);
    }
    hasher.finished()
}

/// Feed `bound`'s length, then each name beside what it is bound to: a type by its handle, a
/// module by its carried type and its tree's identity.
fn feed_binding(hasher: &mut DigestHasher, bound: &[(Unknown, Bound<'_>)]) {
    hasher.count(bound.len());
    for (unknown, bound) in bound {
        hasher.feed(unknown);
        match bound {
            Bound::Type(kt) => hasher.count(0).feed(kt),
            Bound::Module { carried, tree } => hasher.count(1).feed(carried).feed(tree.identity()),
        };
    }
}

/// `old`, a memo at the brand `lay` lays at, with the entry `key` finds extended by `decided`, at
/// the event at `site` past `at`, the last decision the cell reached down `cursor`, the entry it
/// read: a new memo and the entry it now holds, whose [`laid`](Entry::laid) is the decision
/// `decided` laid or joined. A new decision joins the decisions under `at`, and a joining binding
/// the bindings of its decision, the earliest dropped where [`FANOUT`] are held. Only the path
/// from the root is laid again: its siblings are shared by pointer where `cursor` is `old`'s entry
/// for `key`, else dropped and the path relaid. The entry takes the place of `key`'s in `old`.
/// `adopt` is handed every type what is relaid names, for the caller to make resolve at `lay`'s
/// brand.
#[allow(clippy::too_many_arguments)]
pub fn extended<'to, L: Lays<'to>>(
    lay: L,
    old: Option<&'to DecisionMemo<'to>>,
    key: MemoKey,
    cursor: Option<&Entry<'_>>,
    at: Option<&Decision<'_>>,
    site: EventSite,
    decided: Decided<'_>,
    adopt: &mut dyn FnMut(DeclaredType<Parametric>),
) -> (&'to DecisionMemo<'to>, &'to Entry<'to>) {
    let scratch = Bump::new();
    let mut relay = Relay::new(lay, &scratch, Binding::EMPTY);
    let kept_entry = old.and_then(|old| old.find(key));
    // The cell's path, root first.
    let mut path = BumpVec::new_in(&scratch);
    if let Some(at) = at {
        let roots = cursor
            .expect("a decision reached lies in the entry read")
            .roots;
        let found = path_to(roots, at, &mut path);
        debug_assert!(found, "a decision reached lies in the entry read");
    }
    // The path's decisions as `old` holds them, where the entry read is `old`'s own.
    let mut held: BumpVec<'_, &'to Decision<'to>> = BumpVec::new_in(&scratch);
    let shares = match (kept_entry, cursor) {
        (Some(kept), Some(cursor)) => std::ptr::addr_eq(kept, cursor),
        _ => false,
    };
    if let (true, Some(kept)) = (shares, kept_entry) {
        let mut level = kept.roots;
        for read in path.iter() {
            let found = (level.iter().copied())
                .find(|each| std::ptr::addr_eq(*each, *read))
                .expect("a path walks down the entry it read");
            held.push(found);
            level = found.next;
        }
    }
    let siblings = |depth: usize| -> &'to [&'to Decision<'to>] {
        match (shares, kept_entry, depth) {
            (false, _, _) | (_, None, _) => &[],
            (true, Some(kept), 0) => kept.roots,
            (true, Some(_), depth) => held[depth - 1].next,
        }
    };
    let beside = siblings(path.len());
    let (leaf, mut children): (&'to Decision<'to>, &'to [&'to Decision<'to>]) = match decided {
        Decided::New { witnessed, binding } => {
            let witnessed = witnessed.map(|layer| relaid(lay, &scratch, &mut relay, layer, adopt));
            let binding = relaid(lay, &scratch, &mut relay, binding, adopt);
            let leaf = resident(
                lay,
                Decision {
                    site,
                    witnessed: witnessed.map(|layer| resident(lay, layer)),
                    bindings: lay.fill(1, |_| binding),
                    next: &[],
                },
            );
            // The new decision beside the ones held at its event, the earliest dropped past the
            // fanout.
            let kept_beside = &beside[beside.len().saturating_sub(FANOUT - 1)..];
            let children = lay.fill(kept_beside.len() + 1, |at| {
                kept_beside.get(at).copied().unwrap_or(leaf)
            });
            (leaf, children)
        }
        Decided::Joins { into, binding } => {
            // The decision as `old` holds it, shared with its next and its witnessed layer.
            let shared = (beside.iter().copied()).find(|each| std::ptr::addr_eq(*each, into));
            let bindings = &into.bindings[into.bindings.len().saturating_sub(FANOUT - 1)..];
            let binding = relaid(lay, &scratch, &mut relay, binding, adopt);
            let leaf = match shared {
                Some(shared) => resident(
                    lay,
                    Decision {
                        site,
                        witnessed: shared.witnessed,
                        bindings: lay.fill(bindings.len() + 1, |at| {
                            shared.bindings[shared.bindings.len() - bindings.len()..]
                                .get(at)
                                .copied()
                                .unwrap_or(binding)
                        }),
                        next: shared.next,
                    },
                ),
                None => {
                    let held = (into.witnessed, bindings);
                    let (witnessed, laid) = relaid_layers(lay, &scratch, &mut relay, held, adopt);
                    resident(
                        lay,
                        Decision {
                            site,
                            witnessed,
                            bindings: lay.fill(laid.len() + 1, |at| {
                                laid.get(at).copied().unwrap_or(binding)
                            }),
                            next: &[],
                        },
                    )
                }
            };
            let children = match shared {
                Some(shared) => {
                    lay.fill(beside.len(), |at| match std::ptr::eq(beside[at], shared) {
                        true => leaf,
                        false => beside[at],
                    })
                }
                None => lay.fill(1, |_| leaf),
            };
            (leaf, children)
        }
    };
    // Each decision on the path laid again over the children below it.
    for depth in (0..path.len()).rev() {
        let node = match shares {
            true => {
                let held = held[depth];
                resident(
                    lay,
                    Decision {
                        site: held.site,
                        witnessed: held.witnessed,
                        bindings: held.bindings,
                        next: children,
                    },
                )
            }
            false => {
                let read = path[depth];
                let held = (read.witnessed, read.bindings);
                let (witnessed, laid) = relaid_layers(lay, &scratch, &mut relay, held, adopt);
                resident(
                    lay,
                    Decision {
                        site: read.site,
                        witnessed,
                        bindings: collect(lay, laid.iter().copied()),
                        next: children,
                    },
                )
            }
        };
        let level = siblings(depth);
        children = match shares {
            true => lay.fill(level.len(), |at| {
                match std::ptr::eq(level[at], held[depth]) {
                    true => node,
                    false => level[at],
                }
            }),
            false => lay.fill(1, |_| node),
        };
    }
    let entry = resident(
        lay,
        Entry {
            key,
            roots: children,
            laid: Some(leaf),
        },
    );
    let memo = with_entry(lay, old, entry, key.hashed(), 0);
    (memo, entry)
}

/// Whether `to` lies in the trees under `roots`, pushing onto `path` the decisions from its root
/// down to it where it does. A decision lies once in an entry's tree, so the path is its own.
fn path_to<'d>(
    roots: &'d [&'d Decision<'d>],
    to: &Decision<'_>,
    path: &mut BumpVec<'_, &'d Decision<'d>>,
) -> bool {
    let scratch = Bump::new();
    // Each decision still to visit, beside its depth: the path is cut back to it on a visit.
    let mut stack = BumpVec::new_in(&scratch);
    stack.extend(roots.iter().map(|decision| (*decision, 0)));
    while let Some((decision, depth)) = stack.pop() {
        path.truncate(depth);
        path.push(decision);
        if std::ptr::addr_eq(decision, to) {
            return true;
        }
        stack.extend(decision.next.iter().map(|next| (*next, depth + 1)));
    }
    path.clear();
    false
}

/// Hand `each` every decision of the trees under `roots`.
fn each_decision<'a>(roots: &'a [&'a Decision<'a>], each: &mut dyn FnMut(&'a Decision<'a>)) {
    let scratch = Bump::new();
    let mut stack = BumpVec::new_in(&scratch);
    stack.extend(roots.iter().copied());
    while let Some(decision) = stack.pop() {
        each(decision);
        stack.extend(decision.next.iter().copied());
    }
}

/// `layer` laid again alone through `relay`, every type it names handed to `adopt` first.
fn relaid<'to, 'from, L: Lays<'to>>(
    lay: L,
    scratch: &Bump,
    relay: &mut Relay<'to, '_, '_, 'from, L>,
    layer: &BuildLayer<'from>,
    adopt: &mut dyn FnMut(DeclaredType<Parametric>),
) -> BuildLayer<'to> {
    layer.each_own_type(adopt);
    each_bound_type(layer.binding().entries(), adopt);
    layer.relayed(lay, scratch, relay, None)
}

/// A decision's witnessed layer and binding layers, `held`, each laid again through `lay`
/// ([`relaid`]): the witnessed layer resting there, the binding layers staged in `scratch`.
fn relaid_layers<'to, 'x, 'from, L: Lays<'to>>(
    lay: L,
    scratch: &'x Bump,
    relay: &mut Relay<'to, '_, '_, 'from, L>,
    (witnessed, bindings): (Option<&BuildLayer<'from>>, &[BuildLayer<'from>]),
    adopt: &mut dyn FnMut(DeclaredType<Parametric>),
) -> (Option<&'to BuildLayer<'to>>, BumpVec<'x, BuildLayer<'to>>) {
    let witnessed = witnessed.map(|layer| resident(lay, relaid(lay, scratch, relay, layer, adopt)));
    let mut laid = BumpVec::with_capacity_in(bindings.len(), scratch);
    for each in bindings {
        laid.push(relaid(lay, scratch, relay, each, adopt));
    }
    (witnessed, laid)
}

fn each_bound_type(
    bound: &[(Unknown, Bound<'_>)],
    adopt: &mut dyn FnMut(DeclaredType<Parametric>),
) {
    bound
        .iter()
        .for_each(|(_, bound)| adopt_bound(bound, adopt));
}

fn adopt_bound(bound: &Bound<'_>, adopt: &mut dyn FnMut(DeclaredType<Parametric>)) {
    match bound {
        Bound::Type(kt) | Bound::Module { carried: kt, .. } => adopt((*kt).into()),
    }
}
