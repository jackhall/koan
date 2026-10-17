//! How a crossing makes another half's handles resolve in a chain: [`TypeChain::link`] at a spawned
//! child's birth, [`TypeChain::link_or_merge`] at any other birth, [`TypeChain::merge`] at a pinned
//! result or view, and [`TypeChain::adopt`] in a copy; and how a scratch branch reaches its own
//! region's: [`TypeChain::link_late`].
//!
//! A pinned value is embedded as it is, so every handle it carries must resolve in the
//! destination as it did in its home, and nothing may walk the value to learn which ones it
//! carries. Both pin doors rest on one fact: a pinned home outlives the destination, so the home's
//! nodes are valid wherever the destination's are.
//!
//! - A **link** points the destination's branch at the home's, read on a miss and cached, so a
//!   birth costs the same whatever the home holds. A branch links once, as it is laid, and `link`
//!   refuses a laid half, which keeps every branch's depth on its chain fixed: a spawned child
//!   links its spawner's, and any other birth links its home's where it has laid no branch yet, as
//!   a new cell has not. A **late link** sets a laid branch's link where it links nothing and no
//!   branch links it, so no stamp it changes was recorded: a scratch branch, which nothing links,
//!   linked to its region's by the chain that reads both.
//! - A **merge** enters every node of the home's branch in the destination's by reference, laying
//!   nothing, then each branch the home's links reach, up to the first the destination already
//!   reaches. A value of the home may carry a part whose type the home only reached through its
//!   link: reading the part's holder caches the holder's node, not its children's. Each branch
//!   records, as it is laid, its link depth and a tag of its chain's bottom, so the merge knows a
//!   branch the destination reaches by walking the destination's chain only as far as that
//!   branch's depth, and knows a branch on another chain at once unless the two chains' tags
//!   collide, which costs steps, never a wrong answer. A merge pays for how far above the
//!   destination the two chains meet, never for how deep either sits. A hop's predecessor or a
//!   result's child links the destination, and a result forwarded to the reader's home lies in the
//!   branch the reader links, so each of those meets within a link.
//!
//! A copied value is rebuilt in the destination, and its home may die first, so its types are
//! re-laid: [`adopt`](TypeChain::adopt), or [`adopt_from`](TypeChain::adopt_from) reading a whole
//! chain, lays each node the destination lacks at its own digest and
//! flags through the third conversion ([`Content::transfer`]), children first, then makes every
//! handle a side table holds resolve, which closes each cycle through a schema by a hit. A sealed
//! component is re-laid whole, its table already absolute ([`Component::absolute`]). An adopt of a
//! root handle returns at once: every chain over the root resolves it, and its in-root bit says so.
//!
//! See [identity.md § Storage](identity.md#storage-a-root-and-its-branches).

use std::cell::Cell;

use super::chain::{TypeChain, member_content};
use super::component::Component;
use super::digest::{self, ComponentMember};
use super::handle::{DeclaredType, Handle, Parametric};
use super::node::NodeSchema;
use super::stored::{Content, Linked, StoredNode};
use super::tiers::{Branch, BranchTier, Habitat, Tier, lay_one};

impl<'t, H: Habitat<'t>> TypeChain<'t, H> {
    /// Lay the own branch linked to `home`, the spawner's branch a pinned birth crossed from.
    /// Panics where the half has laid a branch, since a branch's depth and bottom are fixed as it
    /// is laid.
    pub fn link<'h: 't>(&self, home: &'h Branch<'h>) {
        let half = self.half();
        assert!(half.own().is_none(), "a branch links as it is laid");
        let tier: &'t dyn Tier<'t> = lay_one(half, BranchTier(home));
        half.own_or_lay().branch().set_link(tier, home.position());
        home.mark_linked();
    }

    /// Link the own branch to `home` where it links nothing yet: a [`link`](Self::link) where the
    /// half has laid no branch, and a late link where it has laid one. Nothing where it links
    /// `home` already.
    ///
    /// A link sets the branch's depth and bottom, which only a branch linking it records, so a
    /// late link panics where another branch links this one; and where `home` reaches this one,
    /// which would close a cycle; and where the branch links another tier. A handle the branch
    /// resolved before resolves as it did: a link is read only on a miss.
    pub fn link_late<'h: 't>(&self, home: &'h Branch<'h>) {
        let half = self.half();
        let Some(own) = half.own() else {
            return self.link(home);
        };
        let branch = own.branch();
        if branch.links(home.position()) {
            return;
        }
        assert!(!branch.links_any(), "a branch links once");
        assert!(
            !branch.is_linked(),
            "a branch another links is linked late, changing the stamps it recorded"
        );
        assert!(
            !home.reaches_branch(branch.address()),
            "a late link closes a cycle"
        );
        let tier: &'t dyn Tier<'t> = lay_one(half, BranchTier(home));
        branch.set_link(tier, home.position());
        home.mark_linked();
    }

    /// Make `home`'s handles resolve at a pinned birth: a [`link`](Self::link) where the half has
    /// laid no branch yet, else a [`merge`](Self::merge). No branch can link an unlaid one, so the
    /// link closes no cycle, and an unlaid branch links nothing, so it is its first. A hop's
    /// `Fresh` successor or a left birth is a new cell, so its birth costs a link whatever the
    /// home reaches.
    pub fn link_or_merge<'h: 't>(&self, home: &'h Branch<'h>) {
        if self.half().own().is_some() {
            self.merge(home);
        } else {
            self.link(home);
        }
    }

    /// Enter every node `source` holds in the own tier, by reference, where the chain does not
    /// already find it, then every node of each branch `source` links, up to the first branch the
    /// chain already reaches, which it finds by depth and bottom, walking its chain only as far as
    /// the depth of the branch it checks. The own branch is laid only if one is entered.
    pub fn merge<'s: 't>(&self, source: &Branch<'s>) {
        let half = self.half();
        let steps = Cell::new(0);
        source.each_until(
            &|at| {
                let reached = self.reaches_at(at, &steps);
                debug_assert_eq!(
                    reached,
                    self.reaches_branch(at.address),
                    "a merge's reach check disagrees with the full walk: a branch's stamps are stale"
                );
                reached
            },
            &mut |node| {
                if self.locate(node.handle()).is_none() {
                    half.own_or_lay().insert(half, node);
                }
            },
        );
        #[cfg(any(test, feature = "counters"))]
        if let Some(super::tiers::Own::Branch(branch)) = half.own() {
            branch.count_reach_steps(steps.get());
        }
    }

    /// Make `handle` resolve in this chain, re-laying in the own tier what the chain lacks, read
    /// from `source` — the copied value's home branch, `None` where the home had none. The root is
    /// shared, so a root handle is found whatever the source.
    pub fn adopt<'s>(&self, source: Option<&Branch<'s>>, kt: impl Into<DeclaredType<Parametric>>) {
        self.adopt_in(&source, kt);
    }

    /// [`adopt`](Self::adopt) reading every tier and link `source` reads — the root, its region
    /// tier, its own tier and their links — so a type whose children lie in several of them
    /// arrives in one pass. Reads `source` without caching into it.
    pub fn adopt_from<'s, H2: Habitat<'s>>(
        &self,
        source: &TypeChain<'s, H2>,
        kt: impl Into<DeclaredType<Parametric>>,
    ) {
        self.adopt_in(&source, kt);
    }

    fn adopt_in<'s>(&self, source: &impl Source<'s>, kt: impl Into<DeclaredType<Parametric>>) {
        let handle = kt.into().raw();
        #[cfg(any(test, feature = "counters"))]
        ADOPT_ASKED.with(|count| count.set(count.get() + 1));
        // A root handle resolves in every chain over the root: a bit test, no lookup.
        if handle.in_root() {
            return;
        }
        #[cfg(any(test, feature = "counters"))]
        ADOPT_PROBES.with(|count| count.set(count.get() + 1));
        self.adopt_handle(source, handle);
    }

    /// [`adopt`](Self::adopt) by handle, answering the node. A root child of a node re-laid here
    /// still takes its one root probe: the re-laid content references the root's node.
    fn adopt_handle<'s>(&self, source: &impl Source<'s>, handle: Handle) -> Linked<'t> {
        if let Some(found) = self.resolve(handle) {
            return found;
        }
        let from = source.reach(handle).unwrap_or_else(|| {
            panic!(
                "a crossed value names type 0x{:032x}, which its home cannot resolve",
                handle.digest().0
            )
        });
        let content = from.content().transfer(
            self.half(),
            &mut |child| self.adopt_handle(source, child.handle()),
            &mut |component| self.adopt_component(source, component),
        );
        let node = self.intern_laid(handle.digest(), || content);
        match node.content() {
            Content::Scheme { group, .. } => {
                for bound in group.bounds {
                    self.adopt_in(source, *bound);
                }
            }
            Content::Signature { schema, .. } => {
                let named = schema
                    .parameters
                    .iter()
                    .chain(schema.manifest_members.iter());
                for (_, kt) in named {
                    self.adopt_in(source, *kt);
                }
                for (_, kt) in schema.value_slots.iter() {
                    self.adopt_in(source, *kt);
                }
                for kt in schema.keyworded {
                    self.adopt_in(source, *kt);
                }
            }
            _ => {}
        }
        node
    }

    /// `from`, a sealed component another half holds, as this chain holds it: the component the
    /// chain already finds, or one re-laid in the own tier with every member interned and each
    /// member schema's handles adopted after, which closes each cycle through a schema by a hit.
    fn adopt_component<'s>(
        &self,
        source: &impl Source<'s>,
        from: &Component<'s>,
    ) -> &'t Component<'t> {
        if let Some(Content::SetMember { component, .. }) = self
            .find(digest::member_ref_digest(from.digest, 0))
            .map(StoredNode::content)
        {
            return component;
        }
        let members = from.members();
        let relaid = self.half().lay(members.len(), |at| ComponentMember {
            schema: self.rehome_schema(members[at].schema),
            ..members[at]
        });
        let component = Component::absolute(self.half(), from.digest, relaid);
        for at in 0..relaid.len() {
            self.intern_laid(digest::member_ref_digest(component.digest, at), || {
                member_content(component, at)
            });
        }
        for member in relaid {
            match member.schema {
                NodeSchema::NewType(repr) => {
                    self.adopt_in(source, repr);
                }
                NodeSchema::TypeConstructor {
                    representation: Some(representation),
                    ..
                } => {
                    self.adopt_in(source, representation);
                }
                NodeSchema::TypeConstructor { .. } => {}
            }
        }
        component
    }
}

/// Where an adopt reads a node it lacks: one branch and its links, or a whole chain.
trait Source<'s> {
    fn reach(&self, handle: Handle) -> Option<Linked<'s>>;
}

impl<'s> Source<'s> for Option<&Branch<'s>> {
    fn reach(&self, handle: Handle) -> Option<Linked<'s>> {
        self.and_then(|branch| branch.reach(handle.digest()))
    }
}

impl<'s, H: Habitat<'s>> Source<'s> for &TypeChain<'s, H> {
    fn reach(&self, handle: Handle) -> Option<Linked<'s>> {
        self.locate(handle).or_else(|| {
            (!handle.in_root())
                .then(|| self.linked_reach(handle.digest()))
                .flatten()
        })
    }
}

#[cfg(any(test, feature = "counters"))]
thread_local! {
    /// How many handles `adopt` and `adopt_from` have been asked to make resolve, a side table's
    /// included. Koan's caching suite reads it.
    pub static ADOPT_ASKED: Cell<usize> = const { Cell::new(0) };
    /// How many of those took a lookup: the ones whose in-root bit is clear.
    pub static ADOPT_PROBES: Cell<usize> = const { Cell::new(0) };
}
