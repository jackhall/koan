//! **Content trees**: what the load knows of a module-valued expression's content, read off the
//! program's text ([design/modules.md § The load and the run](../../design/modules.md#the-load-and-the-run)).
//!
//! A tree's nodes are content — the parts the run digests — and its leaves are what the load cannot
//! see. A **body** is a `MODULE` or `GROUP` body over the trees of what it composes, a **knot** is
//! closures tied into one knot, a **view** is an ascription's operator and application over its
//! operand's tree, a **retype** is a knot member under a header over its bare member's tree, and a
//! **digest** is a value the load computes outright. A **leaf** is keyed on
//! the binder that names it and a value-member chain read through it, and on what says which run
//! of the binder's body holds it: a `MODULE` or `GROUP` body's captures, or the substitution of a
//! call the load unfolds. A leaf no binder names is unlike every tree, itself included.
//!
//! A tree with no leaf is **closed**, and its [digest](Tree::digest) is the one the run computes
//! for the value the expression evaluates to, through the run's own recipes in
//! [`values::digest`](crate::values::digest). Each tree carries an [`Identity`], computed from its
//! children's as it is built: the digest where it is closed, else the same recipes over each
//! leaf's key. Two trees are compared by [`Tree::same_as`] alone, by identity, so a comparison and a
//! digest cost the same however deep the tree or however often a subtree is shared. A layer that
//! computes trees indexes the closed ones by identity through [`closed_trees`], so a module's digest
//! finds the tree its layer computed for it.
//!
//! The load's walk (`dispatch/walk/trees.rs`) builds the trees, through the constructors here.
//! Everything here is `Copy` and names a body by its [`Segment`], never by a reference into program
//! storage, so a tree is covariant in its lifetime: laid in program storage for the load's records,
//! or in a shorter-lived region for a resumption's.

use std::fmt;

use crate::memory::{BumpAllocator, BumpBackedSet, BumpVec, Lays, bump_set};
use crate::symbols::ValueSymbol;
use crate::type_lattice::KType;
use crate::values::ContentDigest;
use crate::values::digest::{
    DigestHasher, Operator, Tag, body_content, edge_at, function_content, knot_digest, lone_member,
    member_digest, retyped_content, view_content,
};

use super::keyed::Keyed;
use super::shape::{BodyShape, Segment, Site, Slot};

/// What the load knows of one module-valued expression's content. Built through the constructors
/// below, which compute each node's [`Identity`].
#[derive(Clone, Copy)]
pub enum Tree<'graph> {
    /// A value the load computes, by its digest: a scalar literal, a closed type value.
    Digest(ContentDigest),
    /// A `MODULE` or `GROUP` body over each capture its content composes, beside its closure slot.
    Body {
        body: Segment,
        captures: &'graph [(u32, Tree<'graph>)],
        identity: Identity,
    },
    /// Member `index` of closures tied into one knot, its nodes in the order the tie lays them.
    Knot {
        members: &'graph [KnotNode<'graph>],
        index: u32,
        identity: Identity,
    },
    /// An opaque view: its operator and the application it is built at, over its operand, digested
    /// over the operand's bare member and what its fit solves.
    View {
        operator: Operator,
        application: KType,
        source: &'graph Tree<'graph>,
        identity: Identity,
    },
    /// A value retyped to `carried` over its bare member's tree: a knot member under a header.
    /// Where the load knows the source's own type it is built only where `carried` differs from
    /// it; otherwise it keys an open source at the type it was retyped to.
    Retyped {
        carried: KType,
        source: &'graph Tree<'graph>,
        identity: Identity,
    },
    Leaf(Leaf<'graph>),
}

/// A tree's identity: the digest the run computes where the tree is closed; the run's recipes
/// over each leaf's key where it is open, so two open trees share an identity exactly where they are
/// alike, leaf for leaf; and nothing where a leaf no binder names sits in it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Identity {
    Closed(ContentDigest),
    Open(ContentDigest),
    Unnamed,
}

/// One closure of a knot: its body, the solution it is an instance at, and each capture its content
/// composes, beside its closure slot.
#[derive(Clone, Copy)]
pub struct KnotNode<'graph> {
    pub body: Segment,
    pub instance: &'graph [KType],
    pub captures: &'graph [(u32, NodeCapture<'graph>)],
}

/// What a closure in a knot captures: a fellow of its knot, by index, or a value's tree.
#[derive(Clone, Copy)]
pub enum NodeCapture<'graph> {
    Fellow(u32),
    Tree(Tree<'graph>),
}

/// What the load cannot see.
#[derive(Clone, Copy)]
pub enum Leaf<'graph> {
    /// The value `binder` holds, read through `chain`'s value members: one value per run of the
    /// binder's body, the run `within` names where it is not the one being typed.
    Bound {
        binder: Binder,
        chain: &'graph [ValueSymbol],
        within: Option<&'graph Within<'graph>>,
        identity: Identity,
    },
    /// An expression no binder names, written at `Site`: unlike every tree.
    Unnamed(Site),
}

/// A binder, by the body declaring it and its slot there.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Binder {
    pub body: Segment,
    pub slot: Slot,
}

/// Which run of a leaf's body holds its binder: a `MODULE` or `GROUP` body's by its builder
/// captures, or a callee's by the call the load unfolded.
#[derive(Clone, Copy)]
pub enum Within<'graph> {
    Body(&'graph [(u32, Tree<'graph>)]),
    Call(Unfolded<'graph>),
}

/// A call the load unfolded: its callee's body, each parameter's argument tree by the callee's
/// parameter slot, and the closed point each of its `FOR ALL` variables solves to, by the
/// variable's slot there.
#[derive(Clone, Copy)]
pub struct Unfolded<'graph> {
    pub callee: Segment,
    pub arguments: &'graph [(Slot, Tree<'graph>)],
    pub solution: &'graph [(Slot, KType)],
}

impl<'graph> Tree<'graph> {
    /// A `MODULE` or `GROUP` body's tree over the captures its content composes.
    pub fn body(shape: &BodyShape<'_>, captures: &'graph [(u32, Tree<'graph>)]) -> Self {
        let identity = Identity::over(
            captures.iter().map(|(_, tree)| tree.identity()),
            |digests| {
                lone_member(body_content(
                    shape.code_digest(),
                    (captures.iter().zip(digests))
                        .map(|((slot, _), digest)| (*slot as usize, digest)),
                ))
            },
        );
        Tree::Body {
            body: shape.segment(),
            captures,
            identity,
        }
    }

    /// Member `index` of the knot `members` tie; `code` is the code digest of each member's body,
    /// by its segment.
    pub fn knot(
        code: &dyn Fn(Segment) -> ContentDigest,
        members: &'graph [KnotNode<'graph>],
        index: u32,
    ) -> Self {
        let captured = members.iter().flat_map(|node| {
            node.captures
                .iter()
                .filter_map(|(_, capture)| match capture {
                    NodeCapture::Fellow(_) => None,
                    NodeCapture::Tree(tree) => Some(tree.identity()),
                })
        });
        let identity = Identity::over(captured, |digests| {
            let contents = members.iter().map(|node| {
                let captures = node.captures.iter().map(|(slot, capture)| {
                    let digest = match capture {
                        NodeCapture::Fellow(fellow) => edge_at(*fellow as usize),
                        NodeCapture::Tree(_) => {
                            digests.next().expect("an identity per tree capture")
                        }
                    };
                    (*slot as usize, digest)
                });
                function_content(code(node.body), node.instance, captures)
            });
            member_digest(knot_digest(members.len(), contents), index as usize)
        });
        Tree::Knot {
            members,
            index,
            identity,
        }
    }

    /// The view `operator` builds at `application` over `source`, whose fit solves `from` where
    /// the load knows it: digested over the source's bare member, and open where `from` is unknown.
    pub fn view(
        operator: Operator,
        application: KType,
        source: &'graph Tree<'graph>,
        from: Option<KType>,
    ) -> Self {
        let identity = Identity::over(std::iter::once(source.bare().identity()), |digests| {
            let source = digests.next().expect("the source's identity");
            lone_member(view_content(operator, application, source, from))
        });
        let identity = match (identity, from) {
            (Identity::Closed(digest), None) => Identity::Open(digest),
            (identity, _) => identity,
        };
        Tree::View {
            operator,
            application,
            source,
            identity,
        }
    }

    /// `source` retyped to `carried`, as the run digests a member under a header.
    pub fn retyped(carried: KType, source: &'graph Tree<'graph>) -> Self {
        let identity = Identity::over(std::iter::once(source.identity()), |digests| {
            let source = digests.next().expect("the source's identity");
            retyped_content(carried.into(), source)
        });
        Tree::Retyped {
            carried,
            source,
            identity,
        }
    }

    /// A leaf on `binder`, read through `chain`, in the run `within` names.
    pub fn bound(
        binder: Binder,
        chain: &'graph [ValueSymbol],
        within: Option<&'graph Within<'graph>>,
    ) -> Self {
        let keyed: &[(_, Tree<'graph>)] = match within {
            None => &[],
            Some(Within::Body(captures)) => captures,
            Some(Within::Call(_)) => &[],
        };
        let arguments: &[(Slot, Tree<'graph>)] = match within {
            Some(Within::Call(call)) => call.arguments,
            _ => &[],
        };
        let children = (keyed.iter().map(|(_, tree)| tree.identity()))
            .chain(arguments.iter().map(|(_, tree)| tree.identity()));
        let identity = match Identity::over(children, |digests| {
            let mut hasher = DigestHasher::new(Tag::Leaf);
            hasher
                .count(binder.body.index() as usize)
                .count(binder.slot.index())
                .count(chain.len());
            for label in chain {
                hasher.feed(label);
            }
            match within {
                None => {
                    hasher.count(0);
                }
                Some(Within::Body(captures)) => {
                    hasher.count(1).count(captures.len());
                }
                Some(Within::Call(call)) => {
                    hasher
                        .count(2)
                        .count(call.callee.index() as usize)
                        .count(call.solution.len());
                    for (slot, point) in call.solution {
                        hasher.count(slot.index()).feed(point);
                    }
                    hasher.count(call.arguments.len());
                }
            }
            let slots = (keyed.iter().map(|(slot, _)| *slot as usize))
                .chain(arguments.iter().map(|(slot, _)| slot.index()));
            for (slot, digest) in slots.zip(digests) {
                hasher.count(slot).digest(digest);
            }
            hasher.finished()
        }) {
            // A leaf is open however closed what keys it.
            Identity::Closed(digest) => Identity::Open(digest),
            identity => identity,
        };
        Tree::Leaf(Leaf::Bound {
            binder,
            chain,
            within,
            identity,
        })
    }

    /// The tree's identity.
    pub fn identity(&self) -> Identity {
        match *self {
            Tree::Digest(digest) => Identity::Closed(digest),
            Tree::Body { identity, .. }
            | Tree::Knot { identity, .. }
            | Tree::View { identity, .. }
            | Tree::Retyped { identity, .. }
            | Tree::Leaf(Leaf::Bound { identity, .. }) => identity,
            Tree::Leaf(Leaf::Unnamed(_)) => Identity::Unnamed,
        }
    }

    /// Whether the tree holds no leaf, a call's key included.
    pub fn closed(&self) -> bool {
        matches!(self.identity(), Identity::Closed(_))
    }

    /// The tree of the bare member behind a header, or this tree where it is none.
    pub fn bare(&self) -> Tree<'graph> {
        match self {
            Tree::Retyped { source, .. } => **source,
            _ => *self,
        }
    }

    /// The digest the run computes for the value this tree's expression evaluates to, where the
    /// tree is closed.
    pub fn digest(&self) -> Option<ContentDigest> {
        match self.identity() {
            Identity::Closed(digest) => Some(digest),
            Identity::Open(_) | Identity::Unnamed => None,
        }
    }

    /// Whether the two trees are one content at every run: closed trees of one digest, or trees of
    /// one structure whose leaves are keyed alike. Never where either holds a leaf no binder names.
    /// The one equality trees have.
    pub fn same_as(&self, other: &Tree<'graph>) -> bool {
        match (self.identity(), other.identity()) {
            (Identity::Unnamed, _) | (_, Identity::Unnamed) => false,
            (mine, theirs) => mine == theirs,
        }
    }

    /// Each tree this one is built over, a leaf's key included.
    fn subtrees(&self, each: &mut impl FnMut(&'graph Tree<'graph>)) {
        match *self {
            Tree::Digest(_) | Tree::Leaf(Leaf::Unnamed(_)) => {}
            Tree::Body { captures, .. } => captures.iter().for_each(|(_, tree)| each(tree)),
            Tree::Knot { members, .. } => {
                for member in members {
                    for (_, capture) in member.captures {
                        if let NodeCapture::Tree(tree) = capture {
                            each(tree);
                        }
                    }
                }
            }
            Tree::View { source, .. } | Tree::Retyped { source, .. } => each(source),
            Tree::Leaf(Leaf::Bound { within, .. }) => match within {
                None => {}
                Some(Within::Body(captures)) => captures.iter().for_each(|(_, tree)| each(tree)),
                Some(Within::Call(unfolded)) => {
                    unfolded.arguments.iter().for_each(|(_, tree)| each(tree))
                }
            },
        }
    }
}

/// Every closed tree among `trees` and their subtrees, keyed by its identity, laid through `lay`:
/// one entry per digest, since two closed trees of one digest are one content. A layer that
/// computes closed trees indexes them so, and a request finds a module's tree by its digest.
pub fn closed_trees<'a>(
    lay: impl Lays<'a>,
    scratch: BumpAllocator<'_>,
    trees: impl Iterator<Item = &'a Tree<'a>>,
) -> Keyed<'a, (ContentDigest, &'a Tree<'a>)> {
    let mut seen: BumpBackedSet<'_, Identity> = bump_set(scratch);
    let mut closed = BumpVec::new_in(scratch);
    let mut pending: BumpVec<'_, &'a Tree<'a>> = BumpVec::new_in(scratch);
    pending.extend(trees);
    while let Some(tree) = pending.pop() {
        let identity = tree.identity();
        // A shared subtree is walked once; a leaf no binder names is unlike every tree.
        if identity != Identity::Unnamed && !seen.insert(identity) {
            continue;
        }
        if let Identity::Closed(digest) = identity {
            closed.push((digest, tree));
        }
        tree.subtrees(&mut |subtree| pending.push(subtree));
    }
    Keyed::sorted(lay, &mut closed)
}

impl Identity {
    /// A node's identity over its `children`'s, in order: `digest` reads each child's digest, or each
    /// child's open key where one is open.
    fn over(
        children: impl Iterator<Item = Identity>,
        digest: impl FnOnce(&mut dyn Iterator<Item = ContentDigest>) -> ContentDigest,
    ) -> Identity {
        let (mut open, mut unnamed) = (false, false);
        let children: smallvec::SmallVec<[ContentDigest; 8]> = children
            .map(|identity| match identity {
                Identity::Closed(digest) => digest,
                Identity::Open(digest) => {
                    open = true;
                    digest
                }
                Identity::Unnamed => {
                    unnamed = true;
                    ContentDigest::NONE
                }
            })
            .collect();
        if unnamed {
            return Identity::Unnamed;
        }
        let digest = digest(&mut children.into_iter());
        match open {
            true => Identity::Open(digest),
            false => Identity::Closed(digest),
        }
    }
}

impl fmt::Debug for Tree<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tree::Digest(digest) => f.debug_tuple("Digest").field(digest).finish(),
            Tree::Body { body, captures, .. } => f
                .debug_struct("Body")
                .field("body", body)
                .field("captures", captures)
                .finish(),
            Tree::Knot { members, index, .. } => f
                .debug_struct("Knot")
                .field("members", &members.len())
                .field("index", index)
                .finish(),
            Tree::View {
                operator,
                application,
                source,
                ..
            } => f
                .debug_struct("View")
                .field("operator", operator)
                .field("application", application)
                .field("source", source)
                .finish(),
            Tree::Retyped {
                carried, source, ..
            } => f
                .debug_struct("Retyped")
                .field("carried", carried)
                .field("source", source)
                .finish(),
            Tree::Leaf(Leaf::Bound { binder, chain, .. }) => f
                .debug_struct("Leaf")
                .field("slot", &binder.slot)
                .field("chain", chain)
                .finish(),
            Tree::Leaf(Leaf::Unnamed(site)) => f.debug_tuple("Unnamed").field(site).finish(),
        }
    }
}
