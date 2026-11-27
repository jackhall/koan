//! **Module content under a binding of the graph**: what [module substitution](super::classes) and
//! [tree application](super::trees) read when a question asked under a binding reads a path class's
//! variable — the class recipes a base's fold laid, the load's records, and the contents the binding
//! gives the names a recipe reads.
//!
//! A class's recipe is read off its root's content tree. Where the binding binds a binder a leaf of
//! that tree holds, the tree is substituted as a fresh load builds it under the binding
//! ([`Apply::substituted`]), and a view over it reads what it binds ([`Apply::view_of`]): a binder
//! binds the module its two names' contents give it, a call unfolds into its callee's laid summary,
//! and a member read through a body is a tree no binder names, which resolves nothing. A class the
//! binding renames another to is filed nowhere: an ask's answer is discrete, so the renamed class
//! never leaves the ask.

use std::cell::Cell;

use crate::elaborate::judge::{self, Derives, Marks};
use crate::knot::module::view::Exposure;
use crate::memory::{BumpAllocator, bump_table, resident};
use crate::scope::{
    Bare, Binder, BinderAt, Bound, Closed, Identity, Leaf, LoadFacts, NameAddress, OpenKind, Root,
    Segment, ShapeError, Site, Slot, StaticCallable, StaticType, Tree, Unknown, rebound,
};
use crate::type_lattice::{DeclaredType, KType, Parametric, PathClass};
use crate::values::{Chain, ContentDigest};

use super::classes::{ClassFacts, ClassOf};
use super::trees::{Apply, Fit, Substitution, TreeFacts};
use super::{Content, GraphFacts, Naming, Writes};

type Read<'o, T> = Result<T, ShapeError<'o>>;

/// What module substitution reads under a binding past the binding itself: the class recipes a
/// base's fold laid, the load's records, and the body whose records the question reading a class
/// belongs to.
#[derive(Clone, Copy)]
pub struct Modules<'m, 'g> {
    pub facts: &'m GraphFacts<'g>,
    pub load: &'m LoadFacts<'g>,
    pub segment: Segment,
}

impl<'g> Modules<'_, 'g> {
    /// The signature every run of the `MODULE` or `GROUP` body `body` ties, where the load typed it
    /// exactly.
    fn signature(&self, types: &Chain<'_, '_, '_, '_>, body: Segment) -> Option<KType> {
        let unit = self.load.shapes().birth_unit(body)?;
        let (facts, _) = self.load.unit(unit)?;
        let (_, typed) = facts.typed.get(body)?;
        judge::static_of(types, typed.signature?).filter(|held| *held != KType::NEVER)
    }

    /// The bare signature and sealing of the module whose knot member's digest is `content`, read
    /// off the tree the load holds for it at `site`, as a frame records them beside its digest.
    pub(super) fn bare<'x>(
        &self,
        types: &'x Chain<'_, '_, '_, 'x>,
        scratch: BumpAllocator<'x>,
        naming: Naming<'_>,
        content: ContentDigest,
        site: Site,
    ) -> Option<Bare<'x>>
    where
        'g: 'x,
    {
        let tree = *self.load.tree_of(content)?;
        let writes = Writes {
            written: &[],
            named: &[],
            modules: Some(*self),
            naming,
        };
        let mut facts = Applying {
            writes: &writes,
            modules: *self,
            types,
            scratch,
        };
        facts.apply().operand_of(tree, site).ok()?.bare
    }

    /// The site a fresh load reads the content tree of the path root `root` from: an inline root's
    /// own, a part held in place's own, a binder's right-hand side or, for one born, its birth.
    fn root_site(&self, root: Root) -> Option<Site> {
        match root {
            Root::Inline { site, .. }
            | Root::Binder(Binder {
                at: BinderAt::Held(site),
                ..
            }) => Some(site),
            Root::Binder(Binder {
                body,
                at: BinderAt::Slot(slot),
            }) => {
                let shape = self.load.shapes().shape_at(body);
                match shape.rhs(slot) {
                    Some(rhs) => Some(shape.site(rhs)),
                    // A parameter's is its body's first statement's, as the walk reads it.
                    None => (shape.birth_site(slot))
                        .or_else(|| shape.body().first().map(|node| shape.site_of_node(node))),
                }
            }
        }
    }
}

/// Module substitution's facts under a binding of the graph: the laid recipes and records, and the
/// binding's contents, with the tree the binding substitutes for the recipe's content tree once
/// applied.
pub(super) struct Substituting<'f, 'w, 'n, 'g, 'x, 'tier, 'pair, 'reg> {
    pub writes: &'f Writes<'w, 'n, 'g>,
    pub modules: Modules<'w, 'g>,
    pub types: &'x Chain<'tier, 'pair, 'reg, 'x>,
    pub scratch: BumpAllocator<'x>,
    /// The content tree of the class being read, beside the site it is read from.
    pub recipe: Cell<Option<(Tree<'g>, Site)>>,
    /// The tree the binding substitutes for the recipe's, where it substitutes one.
    pub applied: Cell<Option<Tree<'x>>>,
}

impl<'f, 'w, 'n, 'g: 'x, 'x, 'tier: 'x, 'pair, 'reg>
    Substituting<'f, 'w, 'n, 'g, 'x, 'tier, 'pair, 'reg>
{
    /// Tree application over the laid facts and the binding.
    fn applying(&self) -> Applying<'f, 'w, 'n, 'g, 'x, 'tier, 'pair, 'reg> {
        Applying {
            writes: self.writes,
            modules: self.modules,
            types: self.types,
            scratch: self.scratch,
        }
    }
}

impl Marks for Substituting<'_, '_, '_, '_, '_, '_, '_, '_> {
    fn open(&self, _: OpenKind, _: Option<DeclaredType<Parametric>>, _: DeclaredType<Parametric>) {}
}

impl Derives for Substituting<'_, '_, '_, '_, '_, '_, '_, '_> {
    type Unwalked = crate::scope::Unwalked;

    fn derive(&self, term: crate::scope::DerivedTerm<'_>) -> Result<Parametric, Self::Unwalked> {
        Ok((self.writes.naming).named(self.types, self.scratch, term))
    }
}

impl<'p, 'o: 'p, 'g: 'x, 'x: 'o, 'tier: 'x> ClassFacts<'p, 'o>
    for Substituting<'_, '_, '_, 'g, 'x, 'tier, '_, '_>
{
    fn class_of(&self, class: PathClass) -> Option<ClassOf<'o>> {
        let (entry, resolution) = self.modules.facts.class(class)?;
        if let Some(site) = self.modules.root_site(entry.root) {
            self.recipe.set(Some((resolution.tree, site)));
            self.applied.set(None);
        }
        Some(ClassOf { entry, resolution })
    }

    fn carried(&self, unknown: Unknown) -> Option<KType> {
        self.writes.given(self.types, unknown)
    }

    fn rebind(&self, _: Segment, ty: DeclaredType<Parametric>) -> Closed {
        rebound(self.types, self.scratch, ty, self.writes)
    }

    fn declared(&self, body: Segment, slot: Slot) -> StaticType<'o> {
        let shape = self.modules.load.shapes().shape_at(body);
        self.modules.load.declared_type(self.types, shape, slot)
    }

    fn signature(&self, body: Segment) -> Option<KType> {
        self.modules.signature(self.types, body)
    }

    /// The recipe's content tree with the binding substituted, where it differs. A binding a fresh
    /// load would refuse there substitutes nothing, and the class is read as the load reads it.
    fn substituted(&self, identity: Identity) -> Option<Tree<'o>> {
        let (tree, site) = self.recipe.get()?;
        if tree.identity() != identity {
            return None;
        }
        if let Some(applied) = self.applied.get() {
            return Some(applied);
        }
        let mut facts = self.applying();
        let mut done = bump_table(self.scratch);
        let applied = (facts.apply())
            .substituted(
                tree,
                Substitution::Binding(self.modules.segment),
                site,
                &mut done,
            )
            .ok()
            .filter(|applied| applied.identity() != identity)?;
        self.applied.set(Some(applied));
        Some(applied)
    }

    fn viewed(&self, identity: Identity) -> Option<Option<Exposure<'o, 'p>>> {
        let applied = self
            .applied
            .get()
            .filter(|tree| tree.identity() == identity)?;
        let (_, site) = self.recipe.get()?;
        let mut facts = self.applying();
        facts.apply().view_of(applied, site).ok()
    }

    fn file_class(&self, _: PathClass, _: ClassOf<'o>) {}
}

/// Tree application's facts under a binding of the graph: the load's laid records, and the
/// binding's contents. Nothing is memoized past one application.
pub(super) struct Applying<'f, 'w, 'n, 'g, 'x, 'tier, 'pair, 'reg> {
    writes: &'f Writes<'w, 'n, 'g>,
    modules: Modules<'w, 'g>,
    types: &'x Chain<'tier, 'pair, 'reg, 'x>,
    scratch: BumpAllocator<'x>,
}

impl<'f, 'w, 'n, 'g: 'x, 'x, 'tier: 'x, 'pair, 'reg>
    Applying<'f, 'w, 'n, 'g, 'x, 'tier, 'pair, 'reg>
{
    fn apply(&mut self) -> Apply<'_, 'x, 'x, 'g, 'tier, 'pair, 'reg, BumpAllocator<'x>, Self> {
        Apply {
            types: self.types,
            scratch: self.scratch,
            lay: self.scratch,
            shapes: self.modules.load.shapes(),
            facts: self,
        }
    }
}

impl Marks for Applying<'_, '_, '_, '_, '_, '_, '_, '_> {
    fn open(&self, _: OpenKind, _: Option<DeclaredType<Parametric>>, _: DeclaredType<Parametric>) {}
}

impl Derives for Applying<'_, '_, '_, '_, '_, '_, '_, '_> {
    type Unwalked = crate::scope::Unwalked;

    fn derive(&self, term: crate::scope::DerivedTerm<'_>) -> Result<Parametric, Self::Unwalked> {
        Ok((self.writes.naming).named(self.types, self.scratch, term))
    }
}

impl<'g: 'x, 'x, 'tier: 'x> TreeFacts<'x, 'x> for Applying<'_, '_, '_, 'g, 'x, 'tier, '_, '_> {
    /// A member read through a body is its laid content where nothing need be written in: the
    /// content is closed, or the body captures nothing. Else it is a tree no binder names: its
    /// laid content is written over the body's own captures, which the tree read through does not
    /// write in.
    fn member(
        &mut self,
        body: Segment,
        captures: &'x [(u32, Tree<'x>)],
        slot: Slot,
        site: Site,
    ) -> Read<'x, Tree<'x>> {
        let load = self.modules.load;
        Ok(match load.content(load.shapes().shape_at(body), slot) {
            Some(laid) if captures.is_empty() || laid.closed() => laid,
            _ => Tree::Leaf(Leaf::Unnamed(site)),
        })
    }

    fn callable(&self, body: Segment) -> StaticCallable<'x> {
        let load = self.modules.load;
        load.callable_type(self.types, load.shapes().shape_at(body))
    }

    fn signature(&self, body: Segment) -> Option<KType> {
        self.modules.signature(self.types, body)
    }

    /// The callee's laid summary: none for a callee in a recursive component.
    fn summary(&mut self, callee: Segment) -> Read<'x, Option<Tree<'x>>> {
        Ok((self.modules.load.closing(callee))
            .filter(|(closing, _)| !closing.recursive)
            .and_then(|(closing, _)| closing.summary.flatten()))
    }

    /// The module the binding binds a binder of `segment`, or of a block around it out to the
    /// first other shape, whose content tree the load holds as one of `identity`: holders of one
    /// tree are one module, as the walk reads them.
    fn held(&self, segment: Segment, identity: Identity) -> Option<Tree<'x>> {
        let (load, shapes) = (self.modules.load, self.modules.load.shapes());
        let mut body = segment;
        loop {
            let shape = shapes.shape_at(body);
            let held = (self.writes.named.iter()).find_map(|named| {
                let NameAddress::Site(unknown) = named.address else {
                    return None;
                };
                let crate::scope::Address::Slot(slot) = unknown.at else {
                    return None;
                };
                let holds = unknown.segment == body
                    && matches!(named.content, Some(Content::Module { .. }))
                    && load.content(shape, slot).map(|tree| tree.identity()) == Some(identity);
                holds.then_some(unknown)
            });
            if let Some(unknown) = held {
                return match self.bound(unknown)? {
                    Bound::Module { tree, .. } => Some(*tree),
                    Bound::Type(_) | Bound::Carried(_) => None,
                };
            }
            if shape.kind() != crate::scope::ShapeKind::Block {
                return None;
            }
            body = shapes.parent(body)?.0;
        }
    }

    /// A type name's closed type, or a module binder's module: its carried name's type beside the
    /// closed tree the load computed for its content name's digest (else the digest with its bare
    /// signature and sealing), retyped where it carries a type other than its bare signature, as a
    /// frame binds a module seen through a header.
    fn bound(&self, unknown: Unknown) -> Option<Bound<'x>> {
        let address = NameAddress::Site(unknown);
        let (mut carried, mut module) = (None, None);
        for named in self.writes.named {
            if named.address != address {
                continue;
            }
            match named.content? {
                Content::Type(DeclaredType::Type(ty)) => {
                    return self.types.concrete(ty).map(Bound::Type);
                }
                Content::Carried(held) => carried = Some(held),
                Content::Module { digest, bare } => module = Some((digest, bare)),
                Content::Type(DeclaredType::Scheme(_)) => return None,
            }
        }
        // A value declared `Any` carries a type and holds no content.
        let ((digest, bare), carried) = match (module, carried?) {
            (Some(module), carried) => (module, carried),
            (None, carried) => return Some(Bound::Carried(carried)),
        };
        let headed = bare.signature != Some(carried);
        // The closed tree the load computed for the module, as a layer stages it, else its digest.
        let load = self.modules.load;
        let mut tree: &'x Tree<'x> = match load.tree_of(digest) {
            Some(tree) => tree,
            None => resident(
                self.scratch,
                Tree::Digest(digest, Some(resident(self.scratch, bare))),
            ),
        };
        if headed {
            tree = resident(self.scratch, Tree::retyped_closed(carried, tree));
        }
        Some(Bound::Module { carried, tree })
    }

    fn rebind(&self, _: Segment, ty: DeclaredType<Parametric>) -> Closed {
        rebound(self.types, self.scratch, ty, self.writes)
    }

    fn fitted(&mut self, _: (KType, Identity)) -> Option<Fit> {
        None
    }

    fn file_fit(&mut self, _: (KType, Identity), _: Fit) {}

    fn viewed(&mut self, _: Identity) -> Option<Option<Exposure<'x, 'x>>> {
        None
    }

    fn file_viewed(&mut self, _: Identity, _: Option<Exposure<'x, 'x>>) {}
}
