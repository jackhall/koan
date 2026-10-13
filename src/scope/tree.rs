//! **Content trees**: what the load knows of a module-valued expression's content, read off the
//! program's text ([design/modules.md § The load and the run](../../design/modules.md#the-load-and-the-run)).
//!
//! A tree's nodes are content — the parts the run digests — and its leaves are what the load cannot
//! see. A **body** is a `MODULE` or `GROUP` body over the trees of what it composes, a **knot** is
//! closures tied into one knot, a **view** is an ascription's operator and application over its
//! operand's tree, and a **digest** is a value the load computes outright. A **leaf** is keyed on
//! the binder that names it and a value-member chain read through it, and on what says which run
//! of the binder's body holds it: a `MODULE` or `GROUP` body's captures, or the substitution of a
//! call the load unfolds. A leaf no binder names is unlike every tree, itself included.
//!
//! A tree with no leaf is **closed**, and its [digest](Tree::digest) is the one the run computes
//! for the value the expression evaluates to, through the run's own recipes in
//! [`values::digest`](crate::values::digest). Each tree carries a [`Print`], computed from its
//! children's as it is built: the digest where it is closed, else the same recipes over each
//! leaf's key. Two trees are compared by [`Tree::same_as`] alone, by print, so a comparison and a
//! digest cost the same however deep the tree or however often a subtree is shared.
//!
//! The static pass ([`dispatch::statics`](../dispatch/statics.rs)) builds the trees, through the
//! constructors here; everything here is `Copy` and laid in program storage, so a shape's cell
//! holds the trees it reads back.

use std::fmt;

use crate::symbols::ValueSymbol;
use crate::type_lattice::KType;
use crate::values::ContentDigest;
use crate::values::digest::{
    DigestHasher, Operator, Tag, body_content, edge_at, function_content, knot_digest, lone_member,
    member_digest, view_content,
};

use super::shape::{BodyShape, Site, Slot};

/// What the load knows of one module-valued expression's content. Built through the constructors
/// below, which compute each node's [`Print`].
#[derive(Clone, Copy)]
pub enum Tree<'graph> {
    /// A value the load computes, by its digest: a scalar literal, a closed type value.
    Digest(ContentDigest),
    /// A `MODULE` or `GROUP` body over each capture its content composes, beside its closure slot.
    Body {
        shape: &'graph BodyShape<'graph>,
        captures: &'graph [(u32, Tree<'graph>)],
        print: Print,
    },
    /// Member `index` of closures tied into one knot, its nodes in the order the tie lays them.
    Knot {
        members: &'graph [KnotNode<'graph>],
        index: u32,
        print: Print,
    },
    /// An ascription's view: its operator and the application it is built at, over its operand.
    View {
        operator: Operator,
        application: KType,
        source: &'graph Tree<'graph>,
        print: Print,
    },
    Leaf(Leaf<'graph>),
}

/// A tree's identity: the digest the run computes where the tree is closed; the run's recipes
/// over each leaf's key where it is open, so two open trees share a print exactly where they are
/// alike, leaf for leaf; and nothing where a leaf no binder names sits in it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Print {
    Closed(ContentDigest),
    Open(ContentDigest),
    Unnamed,
}

/// One closure of a knot: its body, the solution it is an instance at, and each capture its content
/// composes, beside its closure slot.
#[derive(Clone, Copy)]
pub struct KnotNode<'graph> {
    pub shape: &'graph BodyShape<'graph>,
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
        binder: Binder<'graph>,
        chain: &'graph [ValueSymbol],
        within: Option<&'graph Within<'graph>>,
        print: Print,
    },
    /// An expression no binder names, written at `Site`: unlike every tree.
    Unnamed(Site),
}

/// A binder, by the body declaring it and its slot there. Equal by the body's address, which is
/// never part of a digest.
#[derive(Clone, Copy)]
pub struct Binder<'graph> {
    pub shape: &'graph BodyShape<'graph>,
    pub slot: Slot,
}

impl PartialEq for Binder<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.shape, other.shape) && self.slot == other.slot
    }
}

impl Eq for Binder<'_> {}

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
    pub callee: &'graph BodyShape<'graph>,
    pub arguments: &'graph [(Slot, Tree<'graph>)],
    pub solution: &'graph [(Slot, KType)],
}

impl<'graph> Tree<'graph> {
    /// A `MODULE` or `GROUP` body's tree over the captures its content composes.
    pub fn body(shape: &'graph BodyShape<'graph>, captures: &'graph [(u32, Tree<'graph>)]) -> Self {
        let print = Print::over(captures.iter().map(|(_, tree)| tree.print()), |digests| {
            lone_member(body_content(
                shape.code_digest(),
                (captures.iter().zip(digests)).map(|((slot, _), digest)| (*slot as usize, digest)),
            ))
        });
        Tree::Body {
            shape,
            captures,
            print,
        }
    }

    /// Member `index` of the knot `members` tie.
    pub fn knot(members: &'graph [KnotNode<'graph>], index: u32) -> Self {
        let captured = members.iter().flat_map(|node| {
            node.captures
                .iter()
                .filter_map(|(_, capture)| match capture {
                    NodeCapture::Fellow(_) => None,
                    NodeCapture::Tree(tree) => Some(tree.print()),
                })
        });
        let print = Print::over(captured, |digests| {
            let contents = members.iter().map(|node| {
                let captures = node.captures.iter().map(|(slot, capture)| {
                    let digest = match capture {
                        NodeCapture::Fellow(fellow) => edge_at(*fellow as usize),
                        NodeCapture::Tree(_) => digests.next().expect("a print per tree capture"),
                    };
                    (*slot as usize, digest)
                });
                function_content(node.shape.code_digest(), node.instance, captures)
            });
            member_digest(knot_digest(members.len(), contents), index as usize)
        });
        Tree::Knot {
            members,
            index,
            print,
        }
    }

    /// The view `operator` builds at `application` over `source`.
    pub fn view(operator: Operator, application: KType, source: &'graph Tree<'graph>) -> Self {
        let print = Print::over(std::iter::once(source.print()), |digests| {
            let source = digests.next().expect("the source's print");
            lone_member(view_content(operator, application, source))
        });
        Tree::View {
            operator,
            application,
            source,
            print,
        }
    }

    /// A leaf on `binder`, read through `chain`, in the run `within` names.
    pub fn bound(
        binder: Binder<'graph>,
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
        let children = (keyed.iter().map(|(_, tree)| tree.print()))
            .chain(arguments.iter().map(|(_, tree)| tree.print()));
        let print = match Print::over(children, |digests| {
            let mut hasher = DigestHasher::new(Tag::Leaf);
            hasher
                .count(std::ptr::from_ref(binder.shape).addr())
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
                        .count(std::ptr::from_ref(call.callee).addr())
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
            Print::Closed(digest) => Print::Open(digest),
            print => print,
        };
        Tree::Leaf(Leaf::Bound {
            binder,
            chain,
            within,
            print,
        })
    }

    /// The tree's print.
    pub fn print(&self) -> Print {
        match *self {
            Tree::Digest(digest) => Print::Closed(digest),
            Tree::Body { print, .. }
            | Tree::Knot { print, .. }
            | Tree::View { print, .. }
            | Tree::Leaf(Leaf::Bound { print, .. }) => print,
            Tree::Leaf(Leaf::Unnamed(_)) => Print::Unnamed,
        }
    }

    /// Whether the tree holds no leaf, a call's key included.
    pub fn closed(&self) -> bool {
        matches!(self.print(), Print::Closed(_))
    }

    /// The digest the run computes for the value this tree's expression evaluates to, where the
    /// tree is closed.
    pub fn digest(&self) -> Option<ContentDigest> {
        match self.print() {
            Print::Closed(digest) => Some(digest),
            Print::Open(_) | Print::Unnamed => None,
        }
    }

    /// Whether the two trees are one content at every run: closed trees of one digest, or trees of
    /// one structure whose leaves are keyed alike. Never where either holds a leaf no binder names.
    /// The one equality trees have.
    pub fn same_as(&self, other: &Tree<'graph>) -> bool {
        match (self.print(), other.print()) {
            (Print::Unnamed, _) | (_, Print::Unnamed) => false,
            (mine, theirs) => mine == theirs,
        }
    }
}

impl Print {
    /// A node's print over its `children`'s, in order: `digest` reads each child's digest, or each
    /// child's open key where one is open.
    fn over(
        children: impl Iterator<Item = Print>,
        digest: impl FnOnce(&mut dyn Iterator<Item = ContentDigest>) -> ContentDigest,
    ) -> Print {
        let (mut open, mut unnamed) = (false, false);
        let children: smallvec::SmallVec<[ContentDigest; 8]> = children
            .map(|print| match print {
                Print::Closed(digest) => digest,
                Print::Open(digest) => {
                    open = true;
                    digest
                }
                Print::Unnamed => {
                    unnamed = true;
                    ContentDigest::NONE
                }
            })
            .collect();
        if unnamed {
            return Print::Unnamed;
        }
        let digest = digest(&mut children.into_iter());
        match open {
            true => Print::Open(digest),
            false => Print::Closed(digest),
        }
    }
}

impl fmt::Debug for Tree<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tree::Digest(digest) => f.debug_tuple("Digest").field(digest).finish(),
            Tree::Body {
                shape, captures, ..
            } => f
                .debug_struct("Body")
                .field("code", &shape.code_digest())
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
            Tree::Leaf(Leaf::Bound { binder, chain, .. }) => f
                .debug_struct("Leaf")
                .field("slot", &binder.slot)
                .field("chain", chain)
                .finish(),
            Tree::Leaf(Leaf::Unnamed(site)) => f.debug_tuple("Unnamed").field(site).finish(),
        }
    }
}
