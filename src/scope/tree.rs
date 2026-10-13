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
//! [`values::digest`](crate::values::digest). Two trees are compared by [`Tree::same_as`] alone:
//! closed trees by digest, any other structurally.
//!
//! The static pass ([`dispatch::statics`](../dispatch/statics.rs)) builds the trees; everything
//! here is `Copy` and laid in program storage, so a shape's cell holds the trees it reads back.

use std::fmt;

use crate::symbols::ValueSymbol;
use crate::type_lattice::KType;
use crate::values::ContentDigest;
use crate::values::digest::{
    Operator, body_content, edge_at, function_content, knot_digest, lone_member, member_digest,
    view_content,
};

use super::shape::{BodyShape, Site, Slot};

/// What the load knows of one module-valued expression's content.
#[derive(Clone, Copy)]
pub enum Tree<'graph> {
    /// A value the load computes, by its digest: a scalar literal, a closed type value.
    Digest(ContentDigest),
    /// A `MODULE` or `GROUP` body over each capture its content composes, beside its closure slot.
    Body {
        shape: &'graph BodyShape<'graph>,
        captures: &'graph [(u32, Tree<'graph>)],
    },
    /// Member `index` of closures tied into one knot, its nodes in the order the tie lays them.
    Knot {
        members: &'graph [KnotNode<'graph>],
        index: u32,
    },
    /// An ascription's view: its operator and the application it is built at, over its operand.
    View {
        operator: Operator,
        application: KType,
        source: &'graph Tree<'graph>,
    },
    Leaf(Leaf<'graph>),
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
    /// Whether the tree holds no leaf, a call's key included.
    pub fn closed(&self) -> bool {
        match self {
            Tree::Digest(_) => true,
            Tree::Body { captures, .. } => captures.iter().all(|(_, tree)| tree.closed()),
            Tree::Knot { members, .. } => members.iter().all(|node| {
                node.captures.iter().all(|(_, capture)| match capture {
                    NodeCapture::Fellow(_) => true,
                    NodeCapture::Tree(tree) => tree.closed(),
                })
            }),
            Tree::View { source, .. } => source.closed(),
            Tree::Leaf(_) => false,
        }
    }

    /// The digest the run computes for the value this tree's expression evaluates to, where the
    /// tree is closed.
    pub fn digest(&self) -> Option<ContentDigest> {
        self.closed().then(|| self.closed_digest())
    }

    fn closed_digest(&self) -> ContentDigest {
        match *self {
            Tree::Digest(digest) => digest,
            Tree::Body { shape, captures } => lone_member(body_content(
                shape.code_digest(),
                (captures.iter()).map(|(slot, tree)| (*slot as usize, tree.closed_digest())),
            )),
            Tree::Knot { members, index } => {
                let contents = members.iter().map(|node| {
                    let captures = node.captures.iter().map(|(slot, capture)| {
                        let digest = match capture {
                            NodeCapture::Fellow(fellow) => edge_at(*fellow as usize),
                            NodeCapture::Tree(tree) => tree.closed_digest(),
                        };
                        (*slot as usize, digest)
                    });
                    function_content(node.shape.code_digest(), node.instance, captures)
                });
                member_digest(knot_digest(members.len(), contents), index as usize)
            }
            Tree::View {
                operator,
                application,
                source,
            } => lone_member(view_content(operator, application, source.closed_digest())),
            Tree::Leaf(_) => unreachable!("a closed tree holds no leaf"),
        }
    }

    /// Whether a leaf no binder names sits anywhere in the tree, a leaf's key included.
    fn unnamed(&self) -> bool {
        match self {
            Tree::Digest(_) => false,
            Tree::Body { captures, .. } => captures.iter().any(|(_, tree)| tree.unnamed()),
            Tree::Knot { members, .. } => members.iter().any(|node| {
                (node.captures.iter()).any(|(_, capture)| match capture {
                    NodeCapture::Fellow(_) => false,
                    NodeCapture::Tree(tree) => tree.unnamed(),
                })
            }),
            Tree::View { source, .. } => source.unnamed(),
            Tree::Leaf(Leaf::Unnamed(_)) => true,
            Tree::Leaf(Leaf::Bound { within, .. }) => within.is_some_and(|within| match within {
                Within::Body(captures) => captures.iter().any(|(_, tree)| tree.unnamed()),
                Within::Call(call) => call.arguments.iter().any(|(_, tree)| tree.unnamed()),
            }),
        }
    }

    /// Whether the two trees are one content at every run: closed trees of one digest, or trees of
    /// one structure whose leaves are keyed alike. Never where either holds a leaf no binder names.
    /// The one equality trees have.
    pub fn same_as(&self, other: &Tree<'graph>) -> bool {
        if self.unnamed() || other.unnamed() {
            return false;
        }
        if let (Some(mine), Some(theirs)) = (self.digest(), other.digest()) {
            return mine == theirs;
        }
        self.alike(other)
    }

    /// Structural equality, neither side holding an unnamed leaf.
    fn alike(&self, other: &Tree<'graph>) -> bool {
        match (self, other) {
            (Tree::Digest(mine), Tree::Digest(theirs)) => mine == theirs,
            (
                Tree::Body { shape, captures },
                Tree::Body {
                    shape: other,
                    captures: theirs,
                },
            ) => shape.code_digest() == other.code_digest() && captures_alike(captures, theirs),
            (
                Tree::Knot { members, index },
                Tree::Knot {
                    members: theirs,
                    index: at,
                },
            ) => {
                index == at
                    && members.len() == theirs.len()
                    && members.iter().zip(theirs.iter()).all(|(mine, theirs)| {
                        mine.shape.code_digest() == theirs.shape.code_digest()
                            && mine.instance == theirs.instance
                            && mine.captures.len() == theirs.captures.len()
                            && (mine.captures.iter().zip(theirs.captures.iter())).all(
                                |((slot, mine), (at, theirs))| {
                                    slot == at
                                        && match (mine, theirs) {
                                            (NodeCapture::Fellow(a), NodeCapture::Fellow(b)) => {
                                                a == b
                                            }
                                            (NodeCapture::Tree(a), NodeCapture::Tree(b)) => {
                                                a.alike(b)
                                            }
                                            _ => false,
                                        }
                                },
                            )
                    })
            }
            (
                Tree::View {
                    operator,
                    application,
                    source,
                },
                Tree::View {
                    operator: theirs,
                    application: at,
                    source: from,
                },
            ) => operator == theirs && application == at && source.alike(from),
            (
                Tree::Leaf(Leaf::Bound {
                    binder,
                    chain,
                    within,
                }),
                Tree::Leaf(Leaf::Bound {
                    binder: other,
                    chain: theirs,
                    within: under,
                }),
            ) => {
                binder == other
                    && chain == theirs
                    && match (within, under) {
                        (None, None) => true,
                        (Some(Within::Body(mine)), Some(Within::Body(theirs))) => {
                            captures_alike(mine, theirs)
                        }
                        (Some(Within::Call(mine)), Some(Within::Call(theirs))) => {
                            std::ptr::eq(mine.callee, theirs.callee)
                                && mine.solution == theirs.solution
                                && mine.arguments.len() == theirs.arguments.len()
                                && (mine.arguments.iter().zip(theirs.arguments.iter())).all(
                                    |((slot, mine), (at, theirs))| slot == at && mine.alike(theirs),
                                )
                        }
                        _ => false,
                    }
            }
            _ => false,
        }
    }
}

/// Two runs of captures beside their slots, alike slot by slot.
fn captures_alike<'graph>(mine: &[(u32, Tree<'graph>)], theirs: &[(u32, Tree<'graph>)]) -> bool {
    mine.len() == theirs.len()
        && (mine.iter().zip(theirs.iter()))
            .all(|((slot, mine), (at, theirs))| slot == at && mine.alike(theirs))
}

impl fmt::Debug for Tree<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tree::Digest(digest) => f.debug_tuple("Digest").field(digest).finish(),
            Tree::Body { shape, captures } => f
                .debug_struct("Body")
                .field("code", &shape.code_digest())
                .field("captures", captures)
                .finish(),
            Tree::Knot { members, index } => f
                .debug_struct("Knot")
                .field("members", &members.len())
                .field("index", index)
                .finish(),
            Tree::View {
                operator,
                application,
                source,
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
