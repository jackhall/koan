//! How a crossing makes another half's handles resolve in a chain: [`TypeChain::link`] at a spawned
//! child's birth, [`TypeChain::merge`] at every other pin, and [`TypeChain::adopt`] in a copy.
//!
//! A pinned value is embedded as it is, so every handle it carries must resolve in the
//! destination as it did in its home, and nothing may walk the value to learn which ones it
//! carries. Both pin doors rest on one fact: a pinned home outlives the destination, so the home's
//! nodes are valid wherever the destination's are.
//!
//! - A **link** points the destination's branch at the home's, read on a miss and cached, so a
//!   birth costs the same whatever the spawner holds. A cell has one spawner, so a branch links
//!   once.
//! - A **merge** enters every node of the home's branch in the destination's by reference, laying
//!   nothing, then each branch the home's links reach, up to the first the destination already
//!   reaches. A value of the home may carry a part whose type the home only reached through its
//!   link: reading the part's holder caches the holder's node, not its children's. A merge into
//!   the cell the home links stops at once, so only a destination off the home's line of spawners
//!   — a tenant's `Fresh` hop — pays for the walk.
//!
//! A copied value is rebuilt in the destination, and its home may die first, so its types are
//! re-laid: [`adopt`](TypeChain::adopt) lays each node the destination lacks at its own digest and
//! flags through the third conversion ([`Content::transfer`]), children first, then makes every
//! handle a side table holds resolve, which closes each cycle through a schema by a hit. A sealed
//! component is re-laid whole, its table already absolute ([`Component::absolute`]).
//!
//! See [identity.md § Storage](identity.md#storage-a-root-and-its-branches).

use super::chain::{TypeChain, member_content};
use super::component::Component;
use super::digest::{self, ComponentMember, TypeDigest};
use super::handle::{DeclaredType, Parametric};
use super::node::NodeSchema;
use super::stored::{Content, Linked, StoredNode};
use super::tiers::{Branch, BranchTier, Habitat, Tier, lay_one};

impl<'t, H: Habitat<'t>> TypeChain<'t, H> {
    /// Link the own branch to `home`, the spawner's branch a pinned birth crossed from, laying the
    /// own branch if none is laid. Panics on a second link.
    pub fn link<'h: 't>(&self, home: &'h Branch<'h>) {
        let half = self.half();
        let tier: &'t dyn Tier<'t> = lay_one(half, BranchTier(home));
        half.own_or_lay().branch().set_link(tier);
    }

    /// Enter every node `source` holds in the own tier, by reference, where the chain does not
    /// already find it, then every node of each branch `source` links, up to the first branch the
    /// chain already reaches; the own branch is laid only if one is entered.
    pub fn merge<'s: 't>(&self, source: &Branch<'s>) {
        let half = self.half();
        source.each_until(&|address| self.reaches_branch(address), &mut |node| {
            if self.find(node.digest()).is_none() {
                half.own_or_lay().insert(half, node);
            }
        });
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
