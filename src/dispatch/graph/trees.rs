//! **Tree application**: a [content tree](crate::scope::Tree) with a call or a binding written
//! into its leaves, and the views, retypes and member reads over it built again as a fresh load
//! builds them ([module design](../../../design/modules.md#a-call-unfolds)).
//!
//! An application is closed over the tree and what [`TreeFacts`] hands it: the laid facts a tree
//! reads past itself (a callable body's summary and type, a `MODULE` body's signature and member
//! trees) and the binding's content (what a module binder is bound to, and each type closed under
//! it). It types no statement. The load's walk is one caller, whose facts are the records it is
//! writing and which binds nothing; a reader under a binding of the graph is another, over laid
//! records alone.
//!
//! - [`Apply::substituted`] writes a call's arguments and points, or a binding, into a tree, and
//!   [`Apply::unfold`] a call's into its callee's summary; [`Apply::held`] gives the call leaf an
//!   unfolded result is the `LET` binder holding the call whole.
//! - [`Apply::retyped_tree`] and [`Apply::view_tree_held`] build a retype and a `:|` view over a
//!   tree; [`Apply::view_of`] and [`Apply::operand_of`] read what a view binds and the operand a
//!   `:|` reads.
//! - [`Apply::read_chain`] reads a tree through a chain of member labels.
//!
//! A view's fit and what a closed view binds are memoized through the facts, so each caller keeps
//! them where it keeps its other facts.

use crate::elaborate::judge::{self, Derives};
use crate::elaborate::signature_member;
use crate::knot::module::view::{self, Exposure, Operand, Unascribable};
use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect, resident};
use crate::scope::{
    Address, Bare, Binder, BodyShape, Bound, Called, Closed, Identity, KnotNode, Leaf, NodeCapture,
    Position, Revealed, Sealing, Segment, ShapeError, ShapeKind, Shapes, Site, Slot, Static,
    StaticCallable, Tree, Unfolded, Unknown, Within,
};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Side, TypeNode, Variable};
use crate::values::digest::type_digest;
use crate::values::{Chain, retype_target, retypes_by_header};

type Read<'o, T> = Result<T, ShapeError<'o>>;

/// What [`Apply::substituted`] writes into a tree's leaves: a call the load unfolds, or a binding,
/// read from the segment of the record holding the tree, which the binding also writes into each
/// type the tree holds.
#[derive(Clone, Copy)]
pub enum Substitution<'c, 'o> {
    Call(&'c Unfolded<'o>),
    Binding(Segment),
}

/// A view's fit over a source: what it solves the view's head parameters to, the type the source
/// carries, and the member the view shows that its source hides, each where it is known.
pub type Fit = (Option<KType>, Option<KType>, Option<Revealed>);

/// A read an application makes that a test counts.
#[derive(Clone, Copy)]
pub enum Note {
    /// A view fitted over its source's operand.
    Fit,
    /// What a view binds, read.
    ViewRead,
    /// A leaf's chain laid, of this many labels.
    ChainLabels(usize),
}

/// What tree application reads past the tree: laid facts, and the binding's content. The doors it
/// asks mark through the facts' [`Derives`].
pub trait TreeFacts<'p, 'o>: Derives {
    /// The tree of the member at `slot` of the `MODULE` body `body`, read through a tree of the
    /// body whose captures are `captures`, at the part at `site`.
    fn member(
        &mut self,
        body: Segment,
        captures: &'o [(u32, Tree<'o>)],
        slot: Slot,
        site: Site,
    ) -> Read<'o, Tree<'o>>;

    /// What the load fixed for the callable body `body`.
    fn callable(&self, body: Segment) -> StaticCallable<'o>;

    /// The signature every run of the `MODULE` or `GROUP` body `body` ties, where it is known
    /// exactly.
    fn signature(&self, body: Segment) -> Option<KType>;

    /// The summary of the callable body `callee` a call unfolds into: `None` where the call does
    /// not unfold, the callee being recursive or not yet typed.
    fn summary(&mut self, callee: Segment) -> Read<'o, Option<Tree<'o>>>;

    /// The tree the binding binds a module binder of `segment`, or of a shape around it, holding
    /// a tree of `identity` to.
    fn held(&self, segment: Segment, identity: Identity) -> Option<Tree<'o>>;

    /// What the binding gives `unknown`, where it binds it.
    fn bound(&self, unknown: Unknown) -> Option<Bound<'o>>;

    /// `ty`, written over the names of `segment`'s chain, with the binding written in.
    fn rebind(&self, segment: Segment, ty: DeclaredType<Parametric>) -> Closed;

    /// The fit of a view at a closed application over a source of an identity, where it is
    /// memoized.
    fn fitted(&mut self, key: (KType, Identity)) -> Option<Fit>;

    /// Memoize `fit` at `key`.
    fn file_fit(&mut self, key: (KType, Identity), fit: Fit);

    /// What the closed view of `identity` binds, where it is memoized.
    fn viewed(&mut self, identity: Identity) -> Option<Option<Exposure<'o, 'p>>>;

    /// Memoize what the closed view of `identity` binds, where nothing is memoized yet.
    fn file_viewed(&mut self, identity: Identity, viewed: Option<Exposure<'o, 'p>>);

    /// Note a read a test counts.
    fn note(&self, note: Note) {
        let _ = note;
    }
}

/// One application's context: the lattice's chain, the scratch and the program storage it lays
/// in, the program's shapes, and the facts it reads.
pub struct Apply<'f, 'p, 'x, 'g, 'tier, 'pair, 'reg, L, F> {
    pub types: &'x Chain<'tier, 'pair, 'reg, 'x>,
    pub scratch: BumpAllocator<'p>,
    pub lay: L,
    pub shapes: &'g Shapes<'g>,
    pub facts: &'f mut F,
}

impl<'p, 'o: 'p, 'x, 'g, 'tier: 'o, 'pair, 'reg, L: Lays<'o>, F: TreeFacts<'p, 'o>>
    Apply<'_, 'p, 'x, 'g, 'tier, 'pair, 'reg, L, F>
{
    /// `tree`'s value member `label`, read at the part at `site`: through a body, the member's
    /// binding there; through a leaf, the leaf with its chain extended. `None` through anything
    /// else.
    pub fn read_member(
        &mut self,
        tree: Tree<'o>,
        label: ValueSymbol,
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        self.read_step(tree, label, &[], &mut None, site)
    }

    /// `tree` read through each of `labels` in turn, at the part at `site`: `None` where a step
    /// reads nothing. A leaf's chain is extended as a prefix of `labels`, or of one chain joining
    /// the leaf's to them, so the read lays O(n) labels for an n-step chain.
    pub fn read_chain(
        &mut self,
        tree: Tree<'o>,
        labels: &'o [ValueSymbol],
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        let (mut read, mut joined) = (tree, None);
        for at in 0..labels.len() {
            match self.read_step(read, labels[at], &labels[at..], &mut joined, site)? {
                Some(next) => read = next,
                None => return Ok(None),
            }
        }
        Ok(Some(read))
    }

    /// [`read_member`](Self::read_member) as one step of [`read_chain`](Self::read_chain): `rest`
    /// is the chain's labels from `label` on, empty for a lone read, and `joined` the chain the
    /// read laid joining a leaf's chain to them, where it laid one.
    fn read_step(
        &mut self,
        tree: Tree<'o>,
        label: ValueSymbol,
        rest: &'o [ValueSymbol],
        joined: &mut Option<&'o [ValueSymbol]>,
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        Ok(match tree {
            Tree::Body { body, captures, .. } => {
                match self.shapes.shape_at(body).slot(BinderSymbol::Value(label)) {
                    Some((slot, _)) => Some(self.facts.member(body, captures, slot, site)?),
                    None => None,
                }
            }
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
                ..
            }) => Some(Tree::bound(
                binder,
                self.extended(chain, label, rest, joined),
                within,
            )),
            Tree::Leaf(Leaf::Call {
                call,
                holder,
                chain,
                ..
            }) => Some(Tree::called(
                call,
                holder,
                self.extended(chain, label, rest, joined),
            )),
            Tree::Leaf(Leaf::Unnamed(site)) => Some(Tree::Leaf(Leaf::Unnamed(site))),
            // Through a header, the member at the type its carried signature declares it.
            Tree::Retyped {
                carried, source, ..
            } => {
                let (types, scratch) = (self.types, self.scratch);
                let declared = signature_member(
                    &*self.facts,
                    types,
                    scratch,
                    carried,
                    BinderSymbol::Value(label),
                )
                .and_then(|read| read.declared.as_type());
                match (
                    self.read_step(*source, label, rest, joined, site)?,
                    declared,
                ) {
                    (Some(member), Some(declared)) => {
                        Some(self.retyped_tree(member, declared, site)?)
                    }
                    _ => None,
                }
            }
            Tree::Digest(..) | Tree::TypeValue { .. } | Tree::Knot { .. } | Tree::View { .. } => {
                None
            }
        })
    }

    /// A leaf's `chain` extended by `label`, as [`read_step`](Self::read_step) reads it.
    fn extended(
        &self,
        chain: &'o [ValueSymbol],
        label: ValueSymbol,
        rest: &'o [ValueSymbol],
        joined: &mut Option<&'o [ValueSymbol]>,
    ) -> &'o [ValueSymbol] {
        let next = chain.len();
        match *joined {
            // A leaf this read extended already: its chain is a prefix of the joined one.
            Some(joined)
                if std::ptr::eq(chain.as_ptr(), joined.as_ptr())
                    && joined.get(next) == Some(&label) =>
            {
                &joined[..=next]
            }
            _ if rest.is_empty() => {
                let labels = chain.iter().copied().chain([label]);
                self.lay_chain(labels, next + 1)
            }
            _ => {
                let whole = match chain {
                    [] => rest,
                    _ => self.lay_chain(chain.iter().chain(rest).copied(), next + rest.len()),
                };
                *joined = Some(whole);
                &whole[..=next]
            }
        }
    }

    /// The `len` labels of a leaf's chain, laid in program storage.
    fn lay_chain(
        &self,
        labels: impl Iterator<Item = ValueSymbol>,
        len: usize,
    ) -> &'o [ValueSymbol] {
        self.facts.note(Note::ChainLabels(len));
        let mut chain = BumpVec::with_capacity_in(len, self.scratch);
        chain.extend(labels);
        collect(self.lay, chain.iter().copied())
    }

    /// `tree` retyped to `declared` where the run retypes its value — a parameter binding its
    /// argument, a frame's return, an annotated `LET`, a member read through a header. A closure
    /// or a module whose own type the load knows is retyped exactly, under a header only where the
    /// run lays one, and a header's tree is re-retyped over its bare member. An open tree, and any
    /// tree retyped to a `declared` written over names, is keyed on the type it is retyped to,
    /// which the run retypes one value to one way and a binding closes by retyping again; a closed
    /// one whose own type the load does not know is a leaf no binder names, at `site`. A digest is
    /// a scalar, which keeps its type, or a type value, which a kind may re-stamp: its tree is then
    /// such a leaf.
    pub fn retyped_tree(
        &mut self,
        tree: Tree<'o>,
        declared: Parametric,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let unnamed = Tree::Leaf(Leaf::Unnamed(site));
        if let Tree::Leaf(Leaf::Unnamed(_)) = tree {
            return Ok(tree);
        }
        if let Tree::Digest(..) | Tree::TypeValue { .. } = tree {
            // A type value retyped to a kind may take it, and its digest names its kind. A
            // run-bound variable is a kind only as a binding gives it: the tree is keyed at it,
            // which a binding retypes it to closed.
            return Ok(match judge::node_unless_variable(types, declared) {
                Some(TypeNode::OfKind(_)) => unnamed,
                None if judge::names_held(types, scratch, declared) != judge::Names::Unbound => {
                    Tree::retyped(types, declared, resident(self.lay, tree))
                }
                _ => tree,
            });
        }
        // A type written over names keys the tree at it, which a binding retypes it to closed, so
        // reading it closed judges nothing.
        let Some(declared) = judge::static_of(types, declared) else {
            return Ok(Tree::retyped(types, declared, resident(self.lay, tree)));
        };
        if !retypes_by_header(types, declared) {
            return Ok(tree);
        }
        let (bare, own) = match tree {
            Tree::Retyped {
                carried, source, ..
            } => (
                *source,
                // A closed tree's carried type is closed ([`Tree::retyped`]).
                tree.closed()
                    .then(|| judge::static_of(types, carried))
                    .flatten(),
            ),
            Tree::Knot { members, index, .. } => {
                let node = members[index as usize];
                let own = match self.facts.callable(node.body) {
                    Static::Closed(callable) if node.instance.is_empty() => {
                        callable.ktype.as_type()
                    }
                    _ => None,
                };
                (tree, own)
            }
            Tree::Body { .. } | Tree::View { .. } => (tree, self.carried(tree, site)?),
            _ => (tree, None),
        };
        Ok(match own {
            Some(own) => {
                let target = retype_target(types, scratch, own.into(), declared);
                let bare_own = match tree {
                    Tree::Retyped { .. } => None,
                    _ => Some(own),
                };
                match (target.as_type(), bare_own) {
                    (Some(target), Some(bare_own)) if target == bare_own => bare,
                    (Some(target), _) => {
                        Tree::retyped(types, target.into(), resident(self.lay, bare))
                    }
                    (None, _) => unnamed,
                }
            }
            None if tree.closed() => unnamed,
            None => Tree::retyped(types, declared.into(), resident(self.lay, tree)),
        })
    }

    /// The opaque view at `application` over `source`, read at the part at `site`: built over
    /// the source's underlying and keyed on what its fit solves over it, where the load knows
    /// both, and open otherwise — a view of a view that shows a part its source hides among them.
    pub fn view_tree(
        &mut self,
        application: Parametric,
        source: Tree<'o>,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        Ok(self.view_tree_held(application, source, site)?.0)
    }

    /// [`view_tree`](Self::view_tree) beside the type `source` carries, where the load knows it,
    /// and the member the view shows that its source hides, where it does: its fit is memoized by
    /// `application` and the source's identity, and a closed view's [`view_of`](Self::view_of)
    /// seeded from the same fit.
    pub fn view_tree_held(
        &mut self,
        application: Parametric,
        source: Tree<'o>,
        site: Site,
    ) -> Read<'o, (Tree<'o>, Option<KType>, Option<Revealed>)> {
        // An application written over names has no fit until a binding closes it: open, a view a
        // binding substitutes, so reading it closed judges nothing.
        let Some(application) = judge::static_of(self.types, application) else {
            let underlying = resident(self.lay, underlying_tree(self.lay, source));
            let source = resident(self.lay, source);
            return Ok((
                Tree::view(self.types, application, source, underlying, None),
                None,
                None,
            ));
        };
        let key = (application, source.identity());
        let (from, held, revealed, seed) = match self.facts.fitted(key) {
            Some((from, held, revealed)) => (from, held, revealed, None),
            None => {
                self.facts.note(Note::Fit);
                let (types, scratch) = (self.types, self.scratch);
                let operand = self.operand_of(source, site)?;
                let (exposure, revealed) =
                    match view::exposure(operand, application, application, &[], types, scratch) {
                        Ok(exposure) => (Some(exposure), None),
                        Err(Unascribable::Reveals { member }) => (None, Some(member)),
                        Err(_) => (None, None),
                    };
                let from = exposure
                    .and_then(|exposure| exposure.from)
                    .map(|from| from.handle);
                // A leaf no binder names is keyed on its site, which the identity does not tell apart.
                if key.1 != Identity::Unnamed {
                    self.facts.file_fit(key, (from, operand.held, revealed));
                }
                (from, operand.held, revealed, exposure)
            }
        };
        // Built over this source itself, which another of its identity may not be.
        let underlying = underlying_tree(self.lay, source);
        let tree = Tree::view(
            self.types,
            application.into(),
            resident(self.lay, source),
            resident(self.lay, underlying),
            from,
        );
        // A closed view's underlying is closed, so its fit read what the view binds too.
        if let (identity @ Identity::Closed(_), Some(viewed)) = (tree.identity(), seed) {
            self.facts.file_viewed(identity, Some(viewed));
        }
        Ok((tree, held, revealed))
    }

    /// The operand a `:|` over `source` reads: the type it carries, its bare member's digest, and
    /// that member as a view, each where the load knows it — a `MODULE` body a transparent view of
    /// itself, a closed view by what the load computed for it, and a module a frame binds by what
    /// the frame recorded beside its digest.
    pub fn operand_of(&mut self, source: Tree<'o>, site: Site) -> Read<'o, Operand<'p>> {
        let held = self.carried(source, site)?;
        let bare = source.bare();
        let as_view = match bare {
            Tree::Body { .. } => Some(Bare {
                signature: self.carried(bare, site)?,
                sealing: None,
            }),
            Tree::View { underlying, .. } if bare.closed() => {
                let built = self.view_of(bare, site)?.and_then(|viewed| viewed.built);
                let underlying_signature = self.carried(*underlying, site)?;
                match (built, underlying_signature, underlying.digest()) {
                    (Some(built), Some(underlying_signature), Some(digest)) => Some(Bare {
                        signature: Some(built.view),
                        sealing: Some(Sealing {
                            record: built.record,
                            underlying_signature,
                            underlying: digest,
                        }),
                    }),
                    _ => None,
                }
            }
            Tree::Digest(_, Some(bare)) => Some(*bare),
            _ => None,
        };
        Ok(Operand {
            held,
            digest: bare.digest(),
            bare: as_view,
        })
    }

    /// The type a value whose content is `tree` carries, where the load knows it exactly.
    pub fn carried(&mut self, tree: Tree<'o>, site: Site) -> Read<'o, Option<KType>> {
        Ok(match tree {
            Tree::Body { body, .. } => self.facts.signature(body),
            Tree::View { .. } => (self.view_of(tree, site)?)
                .and_then(|viewed| viewed.built)
                .map(|built| built.view),
            // A module a frame binds, by the signature it recorded beside the digest.
            Tree::Digest(_, Some(bare)) => bare.signature,
            // A closed header's type is the one the load computed for it: a closed tree's carried
            // type is closed ([`Tree::retyped`]).
            Tree::Retyped { carried, .. } => tree
                .closed()
                .then(|| judge::static_of(self.types, carried))
                .flatten(),
            _ => None,
        })
    }

    /// What the view `tree` binds, where the load knows it: a closed opaque view's over its
    /// source's operand and its underlying, memoized by identity.
    pub fn view_of(&mut self, tree: Tree<'o>, site: Site) -> Read<'o, Option<Exposure<'o, 'p>>> {
        let Tree::View {
            application,
            source,
            identity,
            ..
        } = tree
        else {
            return Ok(None);
        };
        self.facts.note(Note::ViewRead);
        // An open view's fit or source is unknown (`Tree::view`), so it binds nothing the load
        // knows.
        let Identity::Closed(_) = identity else {
            return Ok(None);
        };
        if let Some(viewed) = self.facts.viewed(identity) {
            return Ok(viewed);
        }
        // A closed view's application is closed ([`Tree::view`]).
        let Some(application) = judge::static_of(self.types, application) else {
            return Ok(None);
        };
        self.facts.note(Note::Fit);
        let operand = self.operand_of(*source, site)?;
        let (types, scratch) = (self.types, self.scratch);
        let viewed = view::exposure(operand, application, application, &[], types, scratch).ok();
        self.facts.file_viewed(identity, viewed);
        Ok(viewed)
    }

    /// `callee`'s summary with `arguments` and `solution` substituted, for a call at `site`:
    /// `None` where the callee has no summary. A solution the load cannot close, `None`, unfolds
    /// only a summary that reads none of it: one over its arguments alone, whatever the points.
    pub fn unfold(
        &mut self,
        callee: &'g BodyShape<'g>,
        arguments: &[(Slot, Tree<'o>)],
        solution: Option<&[(Slot, Parametric)]>,
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        let Some(summary) = self.facts.summary(callee.segment())? else {
            return Ok(None);
        };
        // A type the summary writes over names may name the callee's own `FOR ALL` names, which
        // the call's points would have to be written into.
        let solution = match solution {
            Some(solution) if solution.is_empty() || !self.writes_types(&summary) => solution,
            None if !self.reads_solution(&summary, callee) => &[],
            _ => return Ok(None),
        };
        let call = Unfolded {
            callee: callee.segment(),
            arguments: collect(self.lay, arguments.iter().copied()),
            solution: collect(self.lay, solution.iter().copied()),
        };
        let mut done = bump_table(self.scratch);
        let substituted = self.substituted(summary, Substitution::Call(&call), site, &mut done)?;
        // The frame retypes its value to its declared return.
        let returns = match self
            .callable_type(callee)
            .map(|ty| ty.view(self.types).body)
        {
            Some(TypeNode::KFunction { ret, .. }) => self.at_solution(callee, ret, call.solution),
            _ => None,
        };
        Ok(Some(match returns {
            Some(returns) => self.retyped_tree(substituted, returns, site)?,
            None => substituted,
        }))
    }

    /// `callee`'s function type as the load fixed it ([`callable_type`]).
    fn callable_type(&self, callee: &BodyShape<'_>) -> Option<DeclaredType<Parametric>> {
        callable_type(self.facts.callable(callee.segment()))
    }

    /// `ty`, a position of `callee`'s type, at the call solving its `FOR ALL` variables to
    /// `solution`: each variable written in, or `None` where one has no point there, which a
    /// position read off its scheme would name outside the scheme.
    fn at_solution(
        &self,
        callee: &BodyShape<'_>,
        ty: Parametric,
        solution: &[(Slot, Parametric)],
    ) -> Option<Parametric> {
        let quantified = match self.facts.callable(callee.segment()) {
            Static::Closed(callable) => callable.quantifier_map,
            Static::Rigid { value, .. } => value.quantifier_map,
            Static::Unknown => return None,
        };
        if quantified.is_empty() {
            return Some(ty);
        }
        let mut bindings = BumpVec::with_capacity_in(quantified.iter().len(), self.scratch);
        bindings.resize(quantified.iter().len(), Parametric::from(KType::NEVER));
        for (name, index) in quantified.iter() {
            let (slot, _) = callee.slot(BinderSymbol::Type(name))?;
            let (_, point) = solution.iter().find(|(held, _)| *held == slot)?;
            *bindings.get_mut(index)? = *point;
        }
        Some(crate::type_lattice::substitute_quantified(
            self.types,
            self.scratch,
            ty,
            &bindings,
        ))
    }

    /// Whether `tree`, `callee`'s summary, reads what its call solves: a type written over names, a
    /// type value, which may name `callee`'s own `FOR ALL` names, or a leaf on a binder of `callee`
    /// or a body in it other than a parameter, which a call keys on its solution or reads it from.
    fn reads_solution(&self, tree: &Tree<'o>, callee: &BodyShape<'_>) -> bool {
        if tree.closed() {
            return false;
        }
        if self.writes_types(tree) {
            return true;
        }
        let own = match tree {
            Tree::TypeValue { .. } => true,
            Tree::Leaf(Leaf::Bound { binder, .. }) => {
                let parameter = binder.body == callee.segment()
                    && binder.slot().is_some_and(|slot| {
                        let named = callee.slot_name(slot);
                        matches!(named, BinderSymbol::Value(_))
                            && callee.slot(named).map(|(_, at)| at) == Some(Position::PARAMETER)
                    });
                inside(self.shapes, binder.body, callee.segment()) && !parameter
            }
            _ => false,
        };
        let mut nested = false;
        tree.subtrees(&mut |subtree| nested |= !nested && self.reads_solution(subtree, callee));
        own || nested
    }

    /// Whether `tree` holds a type written over names, in any of its nodes or a call keying a leaf:
    /// a type value aside, whose `FOR ALL` names a call writes its points into.
    fn writes_types(&self, tree: &Tree<'o>) -> bool {
        // A type written over names keeps its node's identity open.
        if tree.closed() {
            return false;
        }
        let open = |ty: &Parametric| judge::static_of(self.types, *ty).is_none();
        let own = match tree {
            Tree::Knot { members, .. } => members.iter().any(|node| node.instance.iter().any(open)),
            Tree::View { application, .. } => open(application),
            Tree::Retyped { carried, .. } => open(carried),
            Tree::Leaf(Leaf::Bound {
                within: Some(Within::Call(call)),
                ..
            }) => call.solution.iter().any(|(_, point)| open(point)),
            _ => false,
        };
        let mut nested = false;
        tree.subtrees(&mut |subtree| nested |= !nested && self.writes_types(subtree));
        own || nested
    }

    /// `tree` with `by` substituted, each view and retype over a substituted leaf built again as
    /// a fresh load builds it. A call: `tree` is a summary of its callee, each parameter's leaf
    /// read by its argument's tree through the leaf's chain, each `FOR ALL` variable's by its
    /// point, and every other leaf of the callee's body keyed on the call. A binding: each leaf on
    /// a binder it binds read by what it binds. `done` holds each subtree substituted already, by
    /// identity, so a shared subtree is substituted once.
    pub fn substituted(
        &mut self,
        tree: Tree<'o>,
        by: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'_, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        let identity = tree.identity();
        // A closed tree holds no leaf to substitute.
        if let Identity::Closed(_) = identity {
            return Ok(tree);
        }
        if let Some(substituted) = done.get(&identity) {
            return Ok(*substituted);
        }
        // A tree a bound module binder holds is the module the binding binds it to.
        if let Substitution::Binding(segment) = by
            && let Some(bound) = self.facts.held(segment, identity)
        {
            done.insert(identity, bound);
            return Ok(bound);
        }
        let substituted = match tree {
            Tree::Digest(..) | Tree::Leaf(Leaf::Unnamed(_)) => tree,
            Tree::TypeValue { named, .. } => {
                type_tree(self.types, self.substituted_type(named, by))
            }
            Tree::Body { body, captures, .. } => Tree::body(
                self.shapes.shape_at(body),
                self.substituted_captures(captures, by, site, done)?,
            ),
            Tree::Knot { members, index, .. } => {
                let mut nodes = BumpVec::with_capacity_in(members.len(), self.scratch);
                for node in members {
                    let mut captures = BumpVec::with_capacity_in(node.captures.len(), self.scratch);
                    for (slot, capture) in node.captures {
                        captures.push((
                            *slot,
                            match capture {
                                NodeCapture::Fellow(fellow) => NodeCapture::Fellow(*fellow),
                                NodeCapture::Tree(tree) => {
                                    NodeCapture::Tree(self.substituted(*tree, by, site, done)?)
                                }
                            },
                        ));
                    }
                    let instance =
                        (node.instance.iter()).map(|solved| self.substituted_type(*solved, by));
                    nodes.push(KnotNode {
                        instance: collect(self.lay, instance),
                        captures: collect(self.lay, captures.iter().copied()),
                        ..*node
                    });
                }
                let shapes = self.shapes;
                Tree::knot(
                    self.types,
                    &|segment| shapes.shape_at(segment).code_digest(),
                    collect(self.lay, nodes.iter().copied()),
                    index,
                )
            }
            Tree::View {
                application,
                source,
                ..
            } => {
                let source = self.substituted(*source, by, site, done)?;
                let application = self.substituted_type(application, by);
                self.view_tree(application, source, site)?
            }
            Tree::Retyped {
                carried, source, ..
            } => {
                let source = self.substituted(*source, by, site, done)?;
                let carried = self.substituted_type(carried, by);
                self.retyped_tree(source, carried, site)?
            }
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
                ..
            }) => self.substituted_leaf(binder, chain, within, by, site, done)?,
            Tree::Leaf(Leaf::Call {
                call: called,
                holder,
                chain,
                ..
            }) => self.substituted_call(called, holder, chain, by, site, done)?,
        };
        if identity != Identity::Unnamed {
            done.insert(identity, substituted);
        }
        Ok(substituted)
    }

    /// [`substituted`](Self::substituted) for a leaf on `binder` read through `chain`, in the run
    /// `within` names.
    fn substituted_leaf(
        &mut self,
        binder: Binder,
        chain: &'o [ValueSymbol],
        within: Option<&'o Within<'o>>,
        by: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'_, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        let call = match by {
            Substitution::Call(call) => call,
            Substitution::Binding(segment) => {
                return self.rebound_leaf(binder, chain, within, segment, site, done);
            }
        };
        if binder.body == call.callee
            && let Some(slot) = binder.slot()
        {
            let callee = self.shapes.shape_at(call.callee);
            if let Some((_, argument)) = (call.arguments.iter()).find(|(each, _)| *each == slot) {
                // The frame binds the argument retyped to the parameter's declared type.
                let declared = match self
                    .callable_type(callee)
                    .map(|ty| ty.view(self.types).body)
                {
                    Some(TypeNode::KFunction { params, .. }) => params
                        .get(callee.slot_name(slot).symbol())
                        .and_then(|declared| self.at_solution(callee, declared, call.solution)),
                    _ => None,
                };
                let argument = match declared {
                    Some(declared) => self.retyped_tree(*argument, declared, site)?,
                    None => *argument,
                };
                if let Some(read) = self.read_chain(argument, chain, site)? {
                    return Ok(read);
                }
            }
        }
        let within = match within {
            Some(within) => Some(self.substituted_within(*within, by, site, done)?),
            // A module body's binder is keyed on its captures, which read nothing.
            None if self.shapes.shape_at(binder.body).kind() == ShapeKind::Module => None,
            None if inside(self.shapes, binder.body, call.callee) => Some(Within::Call(*call)),
            None => None,
        };
        let within = within.map(|within| resident(self.lay, within));
        Ok(Tree::bound(binder, chain, within))
    }

    /// [`substituted_leaf`](Self::substituted_leaf) under a binding: a leaf on a binder it binds,
    /// read in the run being typed, is what the binder is bound to, read through the leaf's chain.
    fn rebound_leaf(
        &mut self,
        binder: Binder,
        chain: &'o [ValueSymbol],
        within: Option<&'o Within<'o>>,
        segment: Segment,
        site: Site,
        done: &mut BumpBackedMap<'_, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        let Some(within) = within else {
            let unknown = binder.unknown();
            let bound = match self.facts.bound(unknown) {
                Some(Bound::Module { tree, .. }) => Some(*tree),
                Some(Bound::Type(bound)) => Some(named_type(self.types, bound)),
                Some(Bound::Carried(_)) | None => None,
            };
            if let Some(bound) = bound
                && let Some(read) = self.read_chain(bound, chain, site)?
            {
                return Ok(read);
            }
            return Ok(Tree::bound(binder, chain, None));
        };
        let within =
            self.substituted_within(*within, Substitution::Binding(segment), site, done)?;
        Ok(Tree::bound(binder, chain, Some(resident(self.lay, within))))
    }

    /// `ty`, a type a tree holds, with `by` written in: the binding read from the record's
    /// segment, or a call's point for each `FOR ALL` name of its callee, which a type name the
    /// callee's body captures holds as its variable.
    fn substituted_type(&self, ty: Parametric, by: Substitution<'_, 'o>) -> Parametric {
        match self.substituted_declared(DeclaredType::Type(ty), by) {
            DeclaredType::Type(ty) => ty,
            DeclaredType::Scheme(_) => ty,
        }
    }

    /// [`substituted_type`](Self::substituted_type) for a declared type.
    fn substituted_declared(
        &self,
        ty: DeclaredType<Parametric>,
        by: Substitution<'_, 'o>,
    ) -> DeclaredType<Parametric> {
        let segment = match by {
            Substitution::Binding(segment) => segment,
            Substitution::Call(call) => return self.at_call(ty, call),
        };
        match self.facts.rebind(segment, ty) {
            Closed::Type(closed) => closed.map(Parametric::from),
            Closed::Partial(partial) => partial,
            Closed::Same => ty,
        }
    }

    /// `ty` with each `FOR ALL` variable of `call`'s callee read as the point the call solves it
    /// to. A scheme is kept: no type a tree holds is one.
    fn at_call(
        &self,
        ty: DeclaredType<Parametric>,
        call: &Unfolded<'o>,
    ) -> DeclaredType<Parametric> {
        let DeclaredType::Type(typed) = ty else {
            return ty;
        };
        let callee = self.shapes.shape_at(call.callee);
        let solved = |level: usize, name: TypeSymbol| {
            (call.solution.iter())
                .find(|(slot, _)| {
                    callee.slot_name(*slot) == BinderSymbol::Type(name)
                        && (self.shapes).level(call.callee, Address::Slot(*slot)) == Some(level)
                })
                .map(|(_, point)| Interval::point(*point))
        };
        let read = judge::read_points(
            self.types,
            self.scratch,
            typed,
            Side::Above,
            &mut |variable| match variable {
                Variable::Lexical { level, name, .. } => solved(level, name),
                _ => None,
            },
        );
        DeclaredType::Type(read)
    }

    /// [`substituted`](Self::substituted) for the result of `called`, held whole by `holder`,
    /// read through `chain`: what a holder the binding binds holds, else the call over its
    /// substituted argument trees, its candidates judged over the types with `by` written in, as
    /// [`resolved_call`](Self::resolved_call) resolves it.
    fn substituted_call(
        &mut self,
        called: &'o Called<'o>,
        holder: Option<Binder>,
        chain: &'o [ValueSymbol],
        by: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'_, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        if let (Substitution::Binding(_), Some(holder)) = (by, holder) {
            let unknown = holder.unknown();
            if let Some(Bound::Module { tree, .. }) = self.facts.bound(unknown)
                && let Some(read) = self.read_chain(*tree, chain, site)?
            {
                return Ok(read);
            }
        }
        let mut arguments = BumpVec::with_capacity_in(called.arguments.len(), self.scratch);
        for tree in called.arguments {
            arguments.push(self.substituted(*tree, by, site, done)?);
        }
        let selection =
            (called.selection).mapped(self.lay, &mut |ty| self.substituted_declared(ty, by));
        // A binder of the callee a call unfolds is bound once per call, which the call keys.
        let holder = match by {
            Substitution::Call(call) => {
                holder.filter(|holder| !inside(self.shapes, holder.body, call.callee))
            }
            Substitution::Binding(_) => holder,
        };
        let called = resident(
            self.lay,
            Called {
                arguments: collect(self.lay, arguments.iter().copied()),
                selection,
                ..*called
            },
        );
        self.resolved_call(called, holder, chain, site)
    }

    /// The tree of the result of `called`, held whole by `holder`, read through `chain`: the
    /// candidate its selection over its types closed selects ([`Selection::selected`]) unfolded, as a fresh load
    /// unfolds the call it selects; else the leaf keyed on the call.
    pub fn resolved_call(
        &mut self,
        called: &'o Called<'o>,
        holder: Option<Binder>,
        chain: &'o [ValueSymbol],
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let leaf = Tree::called(called, holder, chain);
        let selection = called.selection;
        let Some(at) = selection.selected(self.types, self.scratch) else {
            return Ok(leaf);
        };
        let position = selection.candidates[usize::from(at)].position;
        let callee = called.callees[usize::from(position)];
        let Some(body) = callee.body else {
            return Ok(leaf);
        };
        let mut arguments = BumpVec::with_capacity_in(called.arguments.len(), self.scratch);
        arguments.extend(
            callee
                .parameters
                .iter()
                .copied()
                .zip(called.arguments.iter().copied()),
        );
        let solution = (!callee.quantified).then_some(&[][..]);
        let shape = self.shapes.shape_at(body);
        let Some(unfolded) = self.unfold(shape, &arguments, solution, site)? else {
            return Ok(leaf);
        };
        let unfolded = self.held(unfolded, holder);
        Ok(self.read_chain(unfolded, chain, site)?.unwrap_or(leaf))
    }

    /// `tree`, a call's unfolded result held whole by `holder`, with `holder` as the holder of the
    /// call leaf it is, retyped or not, where it holds none: the callee's own call, whose result
    /// the run binds `holder` to. A holder takes no part in a tree's identity.
    pub fn held(&self, tree: Tree<'o>, holder: Option<Binder>) -> Tree<'o> {
        let Some(holder) = holder else {
            return tree;
        };
        let held = |leaf: Tree<'o>| match leaf {
            Tree::Leaf(Leaf::Call {
                call,
                holder: None,
                chain,
                identity,
            }) => Some(Tree::Leaf(Leaf::Call {
                call,
                holder: Some(holder),
                chain,
                identity,
            })),
            _ => None,
        };
        match tree {
            Tree::Retyped {
                carried,
                source,
                identity,
            } => match held(*source) {
                Some(source) => Tree::Retyped {
                    carried,
                    source: resident(self.lay, source),
                    identity,
                },
                None => tree,
            },
            _ => held(tree).unwrap_or(tree),
        }
    }

    /// `within`, which run of a leaf's body holds its binder, with `by` substituted in the trees
    /// that key it.
    fn substituted_within(
        &mut self,
        within: Within<'o>,
        by: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'_, Identity, Tree<'o>>,
    ) -> Read<'o, Within<'o>> {
        Ok(match within {
            Within::Body(captures) => {
                Within::Body(self.substituted_captures(captures, by, site, done)?)
            }
            Within::Call(inner) => {
                let mut arguments = BumpVec::with_capacity_in(inner.arguments.len(), self.scratch);
                for (slot, tree) in inner.arguments {
                    arguments.push((*slot, self.substituted(*tree, by, site, done)?));
                }
                let solution = (inner.solution.iter())
                    .map(|(slot, point)| (*slot, self.substituted_type(*point, by)));
                Within::Call(Unfolded {
                    arguments: collect(self.lay, arguments.iter().copied()),
                    solution: collect(self.lay, solution),
                    ..inner
                })
            }
        })
    }

    fn substituted_captures(
        &mut self,
        captures: &'o [(u32, Tree<'o>)],
        by: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'_, Identity, Tree<'o>>,
    ) -> Read<'o, &'o [(u32, Tree<'o>)]> {
        let mut substituted = BumpVec::with_capacity_in(captures.len(), self.scratch);
        for (slot, tree) in captures {
            substituted.push((*slot, self.substituted(*tree, by, site, done)?));
        }
        Ok(collect(self.lay, substituted.iter().copied()))
    }
}

/// A callable body's function type as the load fixed it, from `callable`, closed or written over
/// the run's variables: `None` where the load fixed none.
pub fn callable_type(callable: StaticCallable<'_>) -> Option<DeclaredType<Parametric>> {
    match callable {
        Static::Closed(callable) => Some(callable.ktype.map(Parametric::from)),
        Static::Rigid { value, .. } => Some(value.ktype),
        Static::Unknown => None,
    }
}

/// The tree of the module a view over `source` is built over: an opaque view's underlying, a
/// module a frame binds that is one by its underlying's digest, and any other module itself,
/// behind any header.
pub fn underlying_tree<'o>(lay: impl Lays<'o>, source: Tree<'o>) -> Tree<'o> {
    match source.bare() {
        Tree::View { underlying, .. } => *underlying,
        Tree::Digest(
            _,
            Some(Bare {
                sealing: Some(sealing),
                ..
            }),
        ) => Tree::Digest(
            sealing.underlying,
            Some(resident(
                lay,
                Bare {
                    signature: Some(sealing.underlying_signature),
                    sealing: None,
                },
            )),
        ),
        bare => bare,
    }
}

/// The tree of a type value naming `named`: its digest where it is closed, else the type value
/// written over names, which a binding closes to that digest.
pub fn type_tree<'o>(types: &Chain<'_, '_, '_, '_>, named: Parametric) -> Tree<'o> {
    match judge::static_of(types, named) {
        Some(closed) => named_type(types, closed),
        None => Tree::type_value(named),
    }
}

/// The tree of a type value naming `named` at its own kind, as the run births it.
pub fn named_type<'o>(types: &Chain<'_, '_, '_, '_>, named: KType) -> Tree<'o> {
    Tree::Digest(
        type_digest(named, KType::of_kind(named.kind_of(types))),
        None,
    )
}

/// Whether `body` is `callee` or a body nested in it, at any depth: a binder bound once per call
/// of `callee`. Walks up from `body`.
pub fn inside(shapes: &Shapes<'_>, mut body: Segment, callee: Segment) -> bool {
    loop {
        if body == callee {
            return true;
        }
        match shapes.parent(body) {
            Some((up, _)) => body = up,
            None => return false,
        }
    }
}
