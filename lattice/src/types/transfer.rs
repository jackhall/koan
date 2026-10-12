//! How a crossing makes another half's handles resolve in a chain: [`TypeChain::link`] at a spawned
//! child's birth, [`TypeChain::link_or_merge`] at any other birth, [`TypeChain::merge`] at a pinned
//! result or view, and [`TypeChain::adopt`] in a copy.
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
//!   a new cell has not.
//! - A **merge** enters every node of the home's branch in the destination's by reference, laying
//!   nothing, then each branch the home's links reach, up to the first the destination already
//!   reaches. A value of the home may carry a part whose type the home only reached through its
//!   link: reading the part's holder caches the holder's node, not its children's. Each branch
//!   records, as it is laid, its link depth and its chain's bottom, so the merge knows a branch the
//!   destination reaches by walking the destination's chain only as far as that branch's depth,
//!   and knows a branch on another chain at once. A merge pays for how far above the destination
//!   the two chains meet, never for how deep either sits. Every door keeps that to a link: a hop's
//!   predecessor or a result's child links the destination, and a result forwarded to the
//!   reader's home lies in the branch the reader links.
//!
//! A copied value is rebuilt in the destination, and its home may die first, so its types are
//! re-laid: [`adopt`](TypeChain::adopt) lays each node the destination lacks at its own digest and
//! flags through the third conversion ([`Content::transfer`]), children first, then makes every
//! handle a side table holds resolve, which closes each cycle through a schema by a hit. A sealed
//! component is re-laid whole, its table already absolute ([`Component::absolute`]).
//!
//! See [identity.md § Storage](identity.md#storage-a-root-and-its-branches).

use std::cell::Cell;

use super::chain::{TypeChain, member_content};
use super::component::Component;
use super::digest::{self, ComponentMember, TypeDigest};
use super::handle::{DeclaredType, Parametric};
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
                if self.find(node.digest()).is_none() {
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
        self.adopt_digest(source, kt.into().raw().digest());
    }

    /// [`adopt`](Self::adopt) by digest, answering the node.
    fn adopt_digest<'s>(&self, source: Option<&Branch<'s>>, digest: TypeDigest) -> Linked<'t> {
        if let Some(found) = self.resolve(digest) {
            return found;
        }
        let from = source
            .and_then(|source| source.reach(digest))
            .unwrap_or_else(|| {
                panic!(
                    "a crossed value names type 0x{:032x}, which its home cannot resolve",
                    digest.0
                )
            });
        let content = from.content().transfer(
            self.half(),
            &mut |child| self.adopt_digest(source, child.digest()),
            &mut |component| self.adopt_component(source, component),
        );
        let node = self.intern_laid(digest, || content);
        match node.content() {
            Content::Scheme { group, .. } => {
                for bound in group.bounds {
                    self.adopt(source, *bound);
                }
            }
            Content::Signature { schema, .. } => {
                let named = schema
                    .parameters
                    .iter()
                    .chain(schema.manifest_members.iter());
                for (_, kt) in named {
                    self.adopt(source, *kt);
                }
                for (_, kt) in schema.value_slots.iter() {
                    self.adopt(source, *kt);
                }
                for kt in schema.keyworded {
                    self.adopt(source, *kt);
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
        source: Option<&Branch<'s>>,
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
                NodeSchema::NewType(repr) => self.adopt(source, repr),
                NodeSchema::TypeConstructor {
                    representation: Some(representation),
                    ..
                } => self.adopt(source, representation),
                NodeSchema::TypeConstructor { .. } => {}
            }
        }
        component
    }
}
