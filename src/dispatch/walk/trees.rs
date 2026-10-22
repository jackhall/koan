//! The static pass's [content trees](crate::scope::Tree): what the load knows of each
//! module-valued expression's content, read off the program's text as the
//! [module design](../../../design/modules.md#the-load-and-the-run) tables it.
//!
//! A tree is read in a **scope**: a shape on the pass's chain, whose captures the chain's own
//! [`follow`](super::Pass::follow) walk resolves, or a `MODULE` body the load reads a member
//! through, whose captures are its tree's and whose top-level reads are the program's. A name is
//! the tree of what its binder holds — a body, a knot of closures, a `LET`'s right-hand side — and
//! a parameter a leaf on its binder. A member read goes through a body to the member's binding and
//! through a leaf by extending its chain. Anything else is a leaf on the `LET` whose whole
//! right-hand side it is, else a leaf no binder names. A leaf on a `MODULE` or `GROUP` body's
//! binder is keyed on the body's builder captures too, wherever it is read, since they say which
//! run of the body its value is from.
//!
//! A call on the chain **unfolds** ([design](../../../design/modules.md#a-call-unfolds)): a
//! keyworded use the pass selected one registration at, or a call by name whose head's tree is a
//! closure, of an unquantified callee or one whose every `FOR ALL` variable solves to a closed
//! point. Its tree is the callee's **summary** — the tree of its body's last statement, its
//! parameters leaves — with each parameter's argument tree and each variable's point substituted,
//! and every other leaf of the callee's keyed on the call. The pass types a callee before the
//! first call it unfolds, on demand where the walk has not reached it yet; a callee met while it
//! is being typed is recursive, as is every callee typed since, and never unfolds.
//!
//! Trees on the chain are memoized per shape and site; a binder already being read reads as a leaf,
//! so no tree is built through itself.

use crate::elaborate::signature_member;
use crate::memory::{BumpBackedMap, BumpVec, bump_table, collect, resident};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    Binder, BodyShape, CaptureSlot, CaptureSource, Coordinate, KnotNode, Leaf, Narrowing,
    NodeCapture, ParameterBinding, Position, Print, Segment, ShapeError, ShapeKind, Site, Slot,
    Static, StaticSolution, Store, Target, Tree, Unfolded, Within,
};
use crate::symbols::{BinderSymbol, ValueSymbol};
use crate::type_lattice::{KType, TypeNode};
use crate::values::digest::{Operator, literal_digest, type_digest};
use crate::values::{retype_target, retypes_by_header};

use super::super::evaluate::{Form, Wanted, of_node, of_part, slots};
use super::{Landing, Pass, address};
use crate::knot::module::view::{self, Ascription};

type Read<'o, T> = Result<T, ShapeError<'o>>;

/// Where a tree is read: a shape on the chain, or a `MODULE` body read through its tree.
#[derive(Clone, Copy)]
pub(super) enum Scope<'o, 'graph> {
    Chain(usize),
    Body {
        shape: &'graph BodyShape<'graph>,
        captures: &'o [(u32, Tree<'o>)],
    },
}

/// The value slot `shape`'s last statement binds, where it binds one: a binding statement's value
/// is what it binds.
pub(super) fn last_binder(shape: &BodyShape<'_>) -> Option<Slot> {
    let last = shape.body().len().checked_sub(1)?;
    (0..shape.slots())
        .map(|slot| Slot(slot as u32))
        .find(|slot| {
            shape.position_of(*slot).statement_index() == Some(last)
                && !matches!(shape.slot_name(*slot), BinderSymbol::Type(_))
                && shape.registration(*slot).is_none()
        })
}

/// Where an expression is written: a part, or a whole node, whose site is its first part's address
/// too, so the two are told apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Written {
    Part(Site),
    Node(Site),
}

impl Written {
    pub(super) fn site(self) -> Site {
        match self {
            Written::Part(site) | Written::Node(site) => site,
        }
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// The shape `scope` reads in.
    fn scope_shape(&self, scope: Scope<'o, 'graph>) -> &'graph BodyShape<'graph> {
        match scope {
            Scope::Chain(level) => self.chain[level].shape,
            Scope::Body { shape, .. } => shape,
        }
    }

    /// A chain level whose chain `scope` reads in: a body read through its tree sits on the chain
    /// being typed.
    fn scope_level(&self, scope: Scope<'o, 'graph>) -> usize {
        match scope {
            Scope::Chain(level) => level,
            Scope::Body { .. } => self.chain.len() - 1,
        }
    }

    /// The tree of `part`, read in the shape at chain level `level`.
    pub(super) fn tree(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Read<'o, Tree<'o>> {
        self.tree_in(Scope::Chain(level), part, None)
    }

    /// The summary of the callable body at chain level `level`: the tree of its last statement.
    pub(super) fn summarize(&mut self, level: usize) -> Read<'o, Option<Tree<'o>>> {
        let shape = self.chain[level].shape;
        let Some(last) = shape.body().len().checked_sub(1) else {
            return Ok(None);
        };
        let scope = Scope::Chain(level);
        let node = &shape.body()[last];
        let tree = match last_binder(shape) {
            Some(slot) => self.slot_tree(scope, slot, Site::of_node(node))?,
            None => {
                let written = Written::Node(Site::of_node(node));
                self.tree_form(scope, of_node(shape, node), written, None)?
            }
        };
        Ok(Some(tree))
    }

    /// The tree of `part` read in `scope`, where `named` is the `LET` binder whose whole right-hand
    /// side it is.
    fn tree_in(
        &mut self,
        scope: Scope<'o, 'graph>,
        part: &'graph ExpressionPart<'graph>,
        named: Option<Slot>,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        self.tree_form(
            scope,
            of_part(shape, part),
            Written::Part(Site::of(part)),
            named,
        )
    }

    /// The tree of what the evaluator reads as `form`, written as `written` in `scope`.
    pub(super) fn tree_form(
        &mut self,
        scope: Scope<'o, 'graph>,
        form: Form<'graph>,
        written: Written,
        named: Option<Slot>,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        if let Scope::Chain(_) = scope
            && let Some((tree, tag)) = self.trees.get(&(address(shape), written)).copied()
        {
            self.gather(tag);
            return Ok(tree);
        }
        let tree = self.tree_read(scope, form, written, named)?;
        if let Scope::Chain(_) = scope {
            let tag = self.innermost();
            self.trees.insert((address(shape), written), (tree, tag));
        }
        Ok(tree)
    }

    /// [`tree_form`](Self::tree_form) unmemoized.
    fn tree_read(
        &mut self,
        scope: Scope<'o, 'graph>,
        form: Form<'graph>,
        written: Written,
        named: Option<Slot>,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        let site = written.site();
        let tree = match form {
            Form::Name(name) => match shape.mention(Site::of(name)) {
                Some(mention) => self.resolve(scope, mention.coordinate, site)?,
                None => self.fallback(scope, named, site)?,
            },
            Form::Member {
                operand,
                label: BinderSymbol::Value(label),
                ..
            } => {
                let through = self.tree_in(scope, operand, None)?;
                let read = self.read_member(through, label, site)?;
                self.or_fallback(read, scope, named, site)?
            }
            Form::Ascribe(node, mode) => {
                let [operand, _, ascribed] = node.parts else {
                    unreachable!("an ascription has an operand, its keyword and a type")
                };
                let typed = self.type_cell(shape, Site::of(&ascribed.value));
                match self.resolved_type(self.scope_level(scope), typed) {
                    Static::Closed(application) => {
                        let source = self.tree_in(scope, &operand.value, None)?;
                        match mode {
                            Ascription::Opaque => self.view_tree(application, source, site)?,
                            // `:!` is a retype.
                            Ascription::Transparent => {
                                let level = self.scope_level(scope);
                                self.retyped_tree(level, source, application.into(), site)?
                            }
                        }
                    }
                    _ => self.fallback(scope, named, site)?,
                }
            }
            Form::Leaf(ExpressionPart::Literal(literal)) => Tree::Digest(literal_digest(literal)),
            Form::Leaf(ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_)) => {
                match self.resolved_type(self.scope_level(scope), self.type_cell(shape, site)) {
                    Static::Closed(handle) => named_type(self.types, handle),
                    _ => self.fallback(scope, named, site)?,
                }
            }
            Form::Lambda(node) => match Site::of_body(node).and_then(|body| shape.nested(body)) {
                Some(body) => {
                    let knot = self.knot(scope, &[(body, false)], 0, site)?;
                    self.or_fallback(knot, scope, named, site)?
                }
                None => self.fallback(scope, named, site)?,
            },
            Form::Call(node, _) => match scope {
                Scope::Chain(level) => {
                    let unfolded = self.keyworded(level, node)?;
                    self.or_fallback(unfolded, scope, named, site)?
                }
                Scope::Body { .. } => self.fallback(scope, named, site)?,
            },
            Form::Apply(node) => match scope {
                Scope::Chain(level) => {
                    let unfolded = self.by_name(level, node)?;
                    self.or_fallback(unfolded, scope, named, site)?
                }
                Scope::Body { .. } => self.fallback(scope, named, site)?,
            },
            _ => self.fallback(scope, named, site)?,
        };
        Ok(tree)
    }

    /// The tree of what `coordinate` holds, read in `scope` at the part at `site`.
    fn resolve(
        &mut self,
        scope: Scope<'o, 'graph>,
        coordinate: Coordinate,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let unnamed = Tree::Leaf(Leaf::Unnamed(site));
        Ok(match scope {
            Scope::Chain(level) => match self.follow(level, coordinate).landing {
                Landing::Slot { level, slot } => self.slot_tree(Scope::Chain(level), slot, site)?,
                Landing::Builtin(_) | Landing::Open { .. } => unnamed,
            },
            Scope::Body { shape, captures } => match coordinate {
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(slot),
                } => self.slot_tree(scope, slot, site)?,
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Capture(capture),
                } => match shape.top_capture(capture) {
                    Some(top) => self.slot_tree(Scope::Chain(0), top, site)?,
                    None => (captures.iter())
                        .find(|(at, _)| *at as usize == capture.index())
                        .map_or(unnamed, |(_, tree)| *tree),
                },
                _ => unnamed,
            },
        })
    }

    /// The tree of what the binder at `slot` of `scope`'s shape holds, read at the part at `site`.
    pub(super) fn slot_tree(
        &mut self,
        scope: Scope<'o, 'graph>,
        slot: Slot,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        let key = (std::ptr::from_ref(shape).addr(), slot);
        if self.reading.contains(&key) {
            return self.leaf_on(scope, slot);
        }
        self.reading.push(key);
        let tree = self.binder_tree(scope, slot, site);
        self.reading.pop();
        tree
    }

    fn binder_tree(
        &mut self,
        scope: Scope<'o, 'graph>,
        slot: Slot,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        if let BinderSymbol::Type(_) = shape.slot_name(slot) {
            return match self.declared_cell(shape, slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Ok(named_type(self.types, handle))
                }
                _ => self.leaf_on(scope, slot),
            };
        }
        if let Some(body) = shape.births(slot) {
            if body.kind() == ShapeKind::Module {
                // A body not yet typed has not laid its type captures down.
                if !self.done(body) {
                    return self.leaf_on(scope, slot);
                }
                let captures = self.captures_of(scope, body, site)?;
                return Ok(Tree::body(body, captures));
            }
            let component = shape.component_of(slot);
            let mut bodies = BumpVec::with_capacity_in(component.members.len(), self.scratch);
            for member in component.members {
                match shape.births(*member) {
                    Some(body) if body.kind() == ShapeKind::Callable => {
                        bodies.push((body, keeps_scheme(shape, *member)));
                    }
                    _ => return self.leaf_on(scope, slot),
                }
            }
            let index = (component.members.iter())
                .position(|member| *member == slot)
                .expect("a binder is a member of its component");
            return match self.knot(scope, &bodies, index as u32, site)? {
                Some(knot) => Ok(knot),
                None => self.leaf_on(scope, slot),
            };
        }
        if shape.position_of(slot) == Position::PARAMETER {
            return self.leaf_on(scope, slot);
        }
        let tree = match shape.rhs(slot) {
            Some(rhs) => self.tree_in(scope, rhs, Some(slot))?,
            None => return self.leaf_on(scope, slot),
        };
        // An annotated `LET` binds its value retyped to the annotation.
        let level = self.scope_level(scope);
        match shape.annotation(slot) {
            Some(annotation) => match self.declared(level, annotation) {
                Some(declared) => self.retyped_tree(level, tree, declared, site),
                None => self.leaf_on(scope, slot),
            },
            None => Ok(tree),
        }
    }

    /// The tree an expression the load cannot see reads as: a leaf on the `LET` binder `named`
    /// whose whole right-hand side it is, else a leaf no binder names, keyed on `site`.
    fn fallback(
        &mut self,
        scope: Scope<'o, 'graph>,
        named: Option<Slot>,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        match named {
            Some(slot) => self.leaf_on(scope, slot),
            None => Ok(Tree::Leaf(Leaf::Unnamed(site))),
        }
    }

    /// `tree`, or where there is none, the [fallback](Self::fallback).
    fn or_fallback(
        &mut self,
        tree: Option<Tree<'o>>,
        scope: Scope<'o, 'graph>,
        named: Option<Slot>,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        match tree {
            Some(tree) => Ok(tree),
            None => self.fallback(scope, named, site),
        }
    }

    /// A leaf on the binder at `slot` of `scope`'s shape. A `MODULE` or `GROUP` body's binder holds
    /// one value per run of the body, and the body's captures say which run, so its leaf is keyed
    /// on them wherever it is read: on the chain, as the shape enclosing the body resolves them,
    /// and through a body's tree, as the tree holds them.
    pub(super) fn leaf_on(&mut self, scope: Scope<'o, 'graph>, slot: Slot) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        let key = match scope {
            _ if shape.kind() != ShapeKind::Module => &[][..],
            Scope::Body { captures, .. } => {
                let builder = shape.captures().len();
                let mut key = BumpVec::with_capacity_in(captures.len(), self.scratch);
                key.extend(
                    captures
                        .iter()
                        .filter(|(at, _)| (*at as usize) < builder)
                        .copied(),
                );
                collect(self.lay, key.iter().copied())
            }
            // A body with no statement reads nothing, so captures nothing.
            Scope::Chain(level) => match shape.body().first() {
                Some(first) => {
                    self.builder_captures(Scope::Chain(level - 1), shape, Site::of_node(first))?
                }
                None => &[][..],
            },
        };
        let within = (!key.is_empty()).then(|| resident(self.lay, Within::Body(key)));
        Ok(Tree::bound(
            Binder {
                body: shape.segment(),
                slot,
            },
            &[],
            within,
        ))
    }

    /// `tree`'s value member `label`, read at the part at `site`: through a body, the member's
    /// binding there; through a leaf, the leaf with its chain extended. `None` through anything
    /// else.
    pub(super) fn read_member(
        &mut self,
        tree: Tree<'o>,
        label: ValueSymbol,
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        Ok(match tree {
            Tree::Body { body, captures, .. } => {
                match self.shape_at(body).slot(BinderSymbol::Value(label)) {
                    Some((slot, _)) => {
                        let shape = self.shape_at(body);
                        Some(self.slot_tree(Scope::Body { shape, captures }, slot, site)?)
                    }
                    None => None,
                }
            }
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
                ..
            }) => {
                let mut extended = BumpVec::with_capacity_in(chain.len() + 1, self.scratch);
                extended.extend(chain.iter().copied());
                extended.push(label);
                let chain = collect(self.lay, extended.iter().copied());
                Some(Tree::bound(binder, chain, within))
            }
            Tree::Leaf(Leaf::Unnamed(site)) => Some(Tree::Leaf(Leaf::Unnamed(site))),
            // Through a header, the member at the type its carried signature declares it.
            Tree::Retyped {
                carried, source, ..
            } => {
                let (types, scratch) = (self.types, self.scratch);
                let declared =
                    signature_member(types, scratch, carried.into(), BinderSymbol::Value(label))
                        .and_then(|read| read.declared.as_type());
                match (self.read_member(*source, label, site)?, declared) {
                    (Some(member), Some(declared)) => {
                        let level = self.chain.len() - 1;
                        Some(self.retyped_tree(level, member, declared, site)?)
                    }
                    _ => None,
                }
            }
            Tree::Digest(_) | Tree::Knot { .. } | Tree::View { .. } => None,
        })
    }

    /// `tree` retyped to `declared` where the run retypes its value — a parameter binding its
    /// argument, a frame's return, an annotated `LET`, a member read through a header. A closure
    /// or a module whose own type the load knows is retyped exactly, under a header only where the
    /// run lays one, and a header's tree is re-retyped over its bare member. An open tree is keyed
    /// on the type it is retyped to, which the run retypes one value to one way; a closed one whose
    /// own type the load does not know, or a `declared` it cannot close, is a leaf no binder names,
    /// at `site`. A digest is a scalar, which keeps its type, or a type value, which a kind may
    /// re-stamp: its tree is then such a leaf.
    pub(super) fn retyped_tree(
        &mut self,
        level: usize,
        tree: Tree<'o>,
        declared: crate::type_lattice::Parametric,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let unnamed = Tree::Leaf(Leaf::Unnamed(site));
        if let Tree::Leaf(Leaf::Unnamed(_)) = tree {
            return Ok(tree);
        }
        if let Tree::Digest(_) = tree {
            // A type value retyped to a kind may take it, and its digest names its kind.
            let kind = matches!(types.node(declared), TypeNode::OfKind(_));
            return Ok(if kind { unnamed } else { tree });
        }
        let Some(declared) = types.concrete(declared) else {
            return Ok(if tree.closed() { unnamed } else { tree });
        };
        if !retypes_by_header(types, declared) {
            return Ok(tree);
        }
        let (bare, own) = match tree {
            Tree::Retyped {
                carried, source, ..
            } => (*source, tree.closed().then_some(carried)),
            Tree::Knot { members, index, .. } => {
                let node = members[index as usize];
                let own = match self
                    .resolved_callable(level, self.callable_cell(self.shape_at(node.body)))
                {
                    Static::Closed(callable) if node.instance.is_empty() => {
                        callable.ktype.as_type()
                    }
                    _ => None,
                };
                (tree, own)
            }
            Tree::Body { .. } | Tree::View { .. } => (
                tree,
                self.carried_type(super::views::Carried::Tree(tree), site)?,
            ),
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
                    (Some(target), _) => Tree::retyped(target, resident(self.lay, bare)),
                    (None, _) => unnamed,
                }
            }
            None if tree.closed() => unnamed,
            None => Tree::retyped(declared, resident(self.lay, tree)),
        })
    }

    /// The opaque view at `application` over `source`, read at the part at `site`: keyed on what
    /// its fit solves where the load knows the source's signature, and open otherwise.
    pub(super) fn view_tree(
        &mut self,
        application: KType,
        source: Tree<'o>,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let held = self.carried_type(super::views::Carried::Tree(source), site)?;
        let from = view::fit_of(held, application, self.types, self.scratch);
        let source = resident(self.lay, source);
        Ok(Tree::view(Operator::Opaque, application, source, from))
    }

    /// The trees of what `body`, born in `scope` at the part at `site`, composes: each capture its
    /// content counts, beside its closure slot — a builder capture's source, and a type capture's.
    fn captures_of(
        &mut self,
        scope: Scope<'o, 'graph>,
        body: &'graph BodyShape<'graph>,
        site: Site,
    ) -> Read<'o, &'o [(u32, Tree<'o>)]> {
        self.composed_captures(scope, body, self.capture_count(body), site)
    }

    /// [`captures_of`](Self::captures_of) short of the type captures, which a module body's `OVER`
    /// list names as builder captures too: what says which run of the body a binder's value is
    /// from, known before the body is typed.
    fn builder_captures(
        &mut self,
        scope: Scope<'o, 'graph>,
        body: &'graph BodyShape<'graph>,
        site: Site,
    ) -> Read<'o, &'o [(u32, Tree<'o>)]> {
        self.composed_captures(scope, body, body.captures().len(), site)
    }

    /// The trees of the first `count` closure slots of `body` its content composes.
    fn composed_captures(
        &mut self,
        scope: Scope<'o, 'graph>,
        body: &'graph BodyShape<'graph>,
        count: usize,
        site: Site,
    ) -> Read<'o, &'o [(u32, Tree<'o>)]> {
        let mut captures = BumpVec::with_capacity_in(count, self.scratch);
        for index in 0..count {
            let capture = CaptureSlot(index as u32);
            if !self.composes(body, capture) {
                continue;
            }
            let tree = match source_of_capture(body, self.laid_captures(body), capture) {
                CaptureSource::Read(coordinate) => self.resolve(scope, coordinate, site)?,
                _ => Tree::Leaf(Leaf::Unnamed(site)),
            };
            captures.push((index as u32, tree));
        }
        Ok(collect(self.lay, captures.iter().copied()))
    }

    /// The knot of closures `bodies` are born as in `scope` at the part at `site`, beside member
    /// `index`; each body beside whether its binder keeps its scheme. `None` where the load cannot
    /// say what solution one is born at.
    fn knot(
        &mut self,
        scope: Scope<'o, 'graph>,
        bodies: &[(&'graph BodyShape<'graph>, bool)],
        index: u32,
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        let mut members = BumpVec::with_capacity_in(bodies.len(), self.scratch);
        for (body, keeps_scheme) in bodies.iter().copied() {
            // A closure's type captures are laid down once its body is typed.
            if !self.typed_on_demand(body)? {
                return Ok(None);
            }
            let Some(instance) = self.born_at(self.scope_level(scope), body, keeps_scheme) else {
                return Ok(None);
            };
            let mut captures = BumpVec::with_capacity_in(self.capture_count(body), self.scratch);
            for slot in 0..self.capture_count(body) {
                let capture = CaptureSlot(slot as u32);
                if !self.composes(body, capture) {
                    continue;
                }
                let held = match source_of_capture(body, self.laid_captures(body), capture) {
                    CaptureSource::Member { index, .. } => NodeCapture::Fellow(index),
                    CaptureSource::Read(coordinate) => {
                        NodeCapture::Tree(self.resolve(scope, coordinate, site)?)
                    }
                    CaptureSource::Hole | CaptureSource::Offered => {
                        NodeCapture::Tree(Tree::Leaf(Leaf::Unnamed(site)))
                    }
                };
                captures.push((slot as u32, held));
            }
            members.push(KnotNode {
                body: body.segment(),
                instance,
                captures: collect(self.lay, captures.iter().copied()),
            });
        }
        Ok(Some(Tree::knot(
            &|segment| self.shapes.shape_at(segment).code_digest(),
            collect(self.lay, members.iter().copied()),
            index,
        )))
    }

    /// The solution the closure `body`, born in the chain `level` sits on, is born at: its born
    /// instance where the load fixed one closed, and nothing where it is born unquantified or, its
    /// binder keeping its scheme, quantified. `None` where the load cannot say.
    fn born_at(
        &self,
        level: usize,
        body: &'graph BodyShape<'graph>,
        keeps_scheme: bool,
    ) -> Option<&'o [KType]> {
        match self.born_instance(body) {
            Some(StaticSolution::Closed(solution)) => Some(solution),
            Some(StaticSolution::Rigid { .. }) => None,
            None => match self.resolved_callable(level, self.callable_cell(body)) {
                Static::Closed(callable) if callable.ktype.as_type().is_some() => Some(&[]),
                Static::Rigid { value, .. } if value.ktype.as_type().is_some() => Some(&[]),
                Static::Closed(_) | Static::Rigid { .. } if keeps_scheme => Some(&[]),
                // A scheme born with no instance is not typed yet, or never typed.
                _ => None,
            },
        }
    }

    /// The tree of the keyworded use `node` of the shape at `level`, unfolded where the pass
    /// selected one registration there whose solve is closed: `None` where it does not unfold.
    fn keyworded(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Read<'o, Option<Tree<'o>>> {
        let shape = self.chain[level].shape;
        let Some(index) = shape.candidate_lists().position(Site::of_node(node)) else {
            return Ok(None);
        };
        let Narrowing::Selected(coordinate) = self.use_at(level, index).narrowing else {
            return Ok(None);
        };
        let followed = self.follow(level, coordinate);
        let Landing::Slot { level: home, slot } = followed.landing else {
            return Ok(None);
        };
        let declaring = self.chain[home].shape;
        let (Some(callee), Some(_)) = (declaring.births(slot), declaring.registration(slot)) else {
            return Ok(None);
        };
        if followed.crossed || callee.kind() != ShapeKind::Callable {
            return Ok(None);
        }
        let Static::Closed(registered) =
            self.resolved_registered(home, self.registered_cell(declaring, slot))
        else {
            return Ok(None);
        };
        let ParameterBinding::Named(names) = registered.parameters else {
            return Ok(None);
        };
        // Each `FOR ALL` variable at the closed point the pass solved it to at this use.
        let mut solution = BumpVec::new_in(self.scratch);
        if !registered.quantifier_map.is_empty() {
            let site = Site::of_node(node);
            let Some(points) = self.selection_at(level, site) else {
                return Ok(None);
            };
            for (name, at) in registered.quantifier_map.iter() {
                let (Some(point), Some((variable, _))) = (
                    points.get(at).copied().flatten(),
                    callee.slot(BinderSymbol::Type(name)),
                ) else {
                    return Ok(None);
                };
                solution.push((variable, point));
            }
        }
        let wanted = slots(node, self.scratch);
        let mut arguments = BumpVec::with_capacity_in(wanted.len(), self.scratch);
        for (position, wanted) in wanted.iter().enumerate() {
            let (Wanted::Evaluated(part), Some(name)) = (wanted, names.get(position)) else {
                return Ok(None);
            };
            let Some((parameter, _)) = callee.slot(*name) else {
                return Ok(None);
            };
            arguments.push((parameter, self.tree(level, part)?));
        }
        self.unfold(callee, &arguments, &solution, Site::of_node(node))
    }

    /// The tree of the call by name `node` of the shape at `level`, unfolded where its head's tree
    /// is an unquantified closure the chain declares and its argument a record literal: `None`
    /// where it does not unfold.
    fn by_name(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Read<'o, Option<Tree<'o>>> {
        let [head, argument] = node.parts else {
            return Ok(None);
        };
        let ExpressionPart::RecordLiteral(fields) = &argument.value else {
            return Ok(None);
        };
        let Tree::Knot { members, index, .. } = self.tree(level, &head.value)? else {
            return Ok(None);
        };
        let callee = self.shape_at(members[index as usize].body);
        let typed = self.resolved_callable(level, self.callable_cell(callee));
        if !matches!(typed, Static::Closed(callable) if callable.ktype.as_type().is_some()) {
            return Ok(None);
        }
        let mut arguments = BumpVec::with_capacity_in(fields.len(), self.scratch);
        for (name, value) in fields.iter() {
            let Some((parameter, position)) = callee.slot(*name) else {
                return Ok(None);
            };
            if position != Position::PARAMETER {
                return Ok(None);
            }
            arguments.push((parameter, self.tree(level, value)?));
        }
        self.unfold(callee, &arguments, &[], Site::of_node(node))
    }

    /// `callee`'s summary with `arguments` and `solution` substituted, for a call at `site`:
    /// `None` where the callee has no summary.
    fn unfold(
        &mut self,
        callee: &'graph BodyShape<'graph>,
        arguments: &[(Slot, Tree<'o>)],
        solution: &[(Slot, KType)],
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        let Some(summary) = self.summary(callee)? else {
            return Ok(None);
        };
        let call = Unfolded {
            callee: callee.segment(),
            arguments: collect(self.lay, arguments.iter().copied()),
            solution: collect(self.lay, solution.iter().copied()),
        };
        let mut done = bump_table(self.scratch);
        let substituted = self.substituted(summary, &call, site, &mut done)?;
        // The frame retypes its value to its declared return.
        let level = self.chain.len() - 1;
        let returns = match self.resolved_callable(level, self.callable_cell(callee)) {
            Static::Closed(callable) => match callable.ktype.view(self.types).body {
                TypeNode::KFunction { ret, .. } => Some(ret),
                _ => None,
            },
            _ => None,
        };
        Ok(Some(match returns {
            Some(returns) => self.retyped_tree(level, substituted, returns, site)?,
            None => substituted,
        }))
    }

    /// The summary of `callee`, a kept call's callee, walking it first where the walk has not:
    /// `None` where the call does not unfold — the callee is still being typed, or recursive — or
    /// where the walk is typing unfilled code.
    fn summary(&mut self, callee: &'graph BodyShape<'graph>) -> Read<'o, Option<Tree<'o>>> {
        if self.unfilled || !self.kept_call(callee)? {
            return Ok(None);
        }
        Ok(self.summary_of(callee).flatten())
    }

    /// Whether the callable body `body` is typed, walking it now where the walk has not reached
    /// it: `false` where it is being typed, where the chain does not hold the shape it is nested
    /// in, and where it is a fellow of a component being typed.
    pub(super) fn typed_on_demand(&mut self, body: &'graph BodyShape<'graph>) -> Read<'o, bool> {
        if self.has_closed(body) {
            return Ok(true);
        }
        if self.unfilled || self.done(body) {
            return Ok(false);
        }
        // The shape `body` is nested in is on the chain, since a read reaches only definitions in
        // scope; the chain above it is set aside while the body is walked.
        let Some(parent) = (0..self.chain.len()).rev().find(|level| {
            (self.chain[*level].shape.nested_shapes().iter())
                .any(|(_, nested)| std::ptr::eq(*nested, body))
        }) else {
            return Ok(false);
        };
        let shape = self.chain[parent].shape;
        let fellow = (self.components.iter()).any(|(level, members)| {
            *level == parent
                && (members.iter()).any(|member| {
                    shape
                        .births(*member)
                        .is_some_and(|born| std::ptr::eq(born, body))
                })
        });
        if fellow {
            return Ok(false);
        }
        let mut stash = BumpVec::with_capacity_in(self.chain.len() - parent - 1, self.scratch);
        stash.extend(self.chain.drain(parent + 1..));
        let walked = self.body(Some(parent), body);
        self.chain.extend(stash);
        walked?;
        Ok(self.has_closed(body))
    }

    /// `tree`, a summary of `call`'s callee, with the call substituted: each parameter's leaf by
    /// its argument's tree read through the leaf's chain, each `FOR ALL` variable's by its point,
    /// and every other leaf of the callee's body keyed on the call. `done` holds each subtree
    /// substituted for this call already, by print, so a subtree shared across the summary is
    /// substituted once.
    fn substituted(
        &mut self,
        tree: Tree<'o>,
        call: &Unfolded<'o>,
        site: Site,
        done: &mut BumpBackedMap<'p, Print, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        let print = tree.print();
        // A closed tree holds no leaf to substitute.
        if let Print::Closed(_) = print {
            return Ok(tree);
        }
        if let Some(substituted) = done.get(&print) {
            return Ok(*substituted);
        }
        let substituted = match tree {
            Tree::Digest(_) | Tree::Leaf(Leaf::Unnamed(_)) => tree,
            Tree::Body { body, captures, .. } => Tree::body(
                self.shape_at(body),
                self.substituted_captures(captures, call, site, done)?,
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
                                    NodeCapture::Tree(self.substituted(*tree, call, site, done)?)
                                }
                            },
                        ));
                    }
                    nodes.push(KnotNode {
                        captures: collect(self.lay, captures.iter().copied()),
                        ..*node
                    });
                }
                Tree::knot(
                    &|segment| self.shapes.shape_at(segment).code_digest(),
                    collect(self.lay, nodes.iter().copied()),
                    index,
                )
            }
            Tree::View {
                application,
                source,
                ..
            } => {
                let source = self.substituted(*source, call, site, done)?;
                self.view_tree(application, source, site)?
            }
            Tree::Retyped {
                carried, source, ..
            } => {
                let source = self.substituted(*source, call, site, done)?;
                let level = self.chain.len() - 1;
                self.retyped_tree(level, source, carried.into(), site)?
            }
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
                ..
            }) => self.substituted_leaf(binder, chain, within, call, site, done)?,
        };
        if print != Print::Unnamed {
            done.insert(print, substituted);
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
        call: &Unfolded<'o>,
        site: Site,
        done: &mut BumpBackedMap<'p, Print, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        if binder.body == call.callee {
            let callee = self.shape_at(call.callee);
            if let Some((_, argument)) =
                (call.arguments.iter()).find(|(slot, _)| *slot == binder.slot)
            {
                // The frame binds the argument retyped to the parameter's declared type.
                let level = self.chain.len() - 1;
                let declared = match self.resolved_callable(level, self.callable_cell(callee)) {
                    Static::Closed(callable) => match callable.ktype.view(self.types).body {
                        TypeNode::KFunction { params, .. } => {
                            params.get(callee.slot_name(binder.slot).symbol())
                        }
                        _ => None,
                    },
                    _ => None,
                };
                let argument = match declared {
                    Some(declared) => self.retyped_tree(level, *argument, declared, site)?,
                    None => *argument,
                };
                let mut read = Some(argument);
                for label in chain {
                    read = match read {
                        Some(tree) => self.read_member(tree, *label, site)?,
                        None => None,
                    };
                }
                if let Some(read) = read {
                    return Ok(read);
                }
            }
            if let Some((_, point)) = (call.solution.iter()).find(|(slot, _)| *slot == binder.slot)
                && chain.is_empty()
            {
                return Ok(named_type(self.types, *point));
            }
        }
        let within = match within {
            Some(Within::Body(captures)) => Some(Within::Body(
                self.substituted_captures(captures, call, site, done)?,
            )),
            Some(Within::Call(inner)) => {
                let mut arguments = BumpVec::with_capacity_in(inner.arguments.len(), self.scratch);
                for (slot, tree) in inner.arguments {
                    arguments.push((*slot, self.substituted(*tree, call, site, done)?));
                }
                Some(Within::Call(Unfolded {
                    arguments: collect(self.lay, arguments.iter().copied()),
                    ..*inner
                }))
            }
            // A module body's binder is keyed on its captures, which read nothing.
            None if self.shape_at(binder.body).kind() == ShapeKind::Module => None,
            None if inside(binder.body, self.shape_at(call.callee)) => Some(Within::Call(*call)),
            None => None,
        };
        let within = within.map(|within| resident(self.lay, within));
        Ok(Tree::bound(binder, chain, within))
    }

    fn substituted_captures(
        &mut self,
        captures: &'o [(u32, Tree<'o>)],
        call: &Unfolded<'o>,
        site: Site,
        done: &mut BumpBackedMap<'p, Print, Tree<'o>>,
    ) -> Read<'o, &'o [(u32, Tree<'o>)]> {
        let mut substituted = BumpVec::with_capacity_in(captures.len(), self.scratch);
        for (slot, tree) in captures {
            substituted.push((*slot, self.substituted(*tree, call, site, done)?));
        }
        Ok(collect(self.lay, substituted.iter().copied()))
    }
}

/// The tree of a type value naming `named` at its own kind, as the run births it.
fn named_type<'o>(types: &crate::values::Chain<'_, '_, '_, '_>, named: KType) -> Tree<'o> {
    Tree::Digest(type_digest(named, KType::of_kind(named.kind_of(types))))
}

/// Whether `shape` is `callee` or a body nested in it, at any depth: a binder bound once per call
/// of `callee`.
fn inside(body: Segment, callee: &BodyShape<'_>) -> bool {
    // Every body a binder births is a nested shape too.
    body == callee.segment()
        || (callee.nested_shapes().iter()).any(|(_, nested)| inside(body, nested))
}

/// Where capture `capture` of `body`, whose type captures are `laid`, is read from at birth: a
/// builder capture's source, or a type capture's coordinate.
fn source_of_capture(
    body: &BodyShape<'_>,
    laid: &[Coordinate],
    capture: CaptureSlot,
) -> CaptureSource {
    let builder = body.captures();
    match builder.get(capture.index()) {
        Some(spec) => spec.source,
        None => CaptureSource::Read(laid[capture.index() - builder.len()]),
    }
}

/// Whether the binder at `slot` of `shape` keeps the scheme of the quantified callable it births,
/// which is then born quantified: a registration, or a `MODULE` or `GROUP` body's plain `LET`.
fn keeps_scheme(shape: &BodyShape<'_>, slot: Slot) -> bool {
    if shape.registration(slot).is_some() {
        return true;
    }
    shape.kind() == ShapeKind::Module
        && shape
            .position_of(slot)
            .statement_index()
            .is_some_and(|index| {
                shape.body()[index]
                    .cache()
                    .builtin_shape()
                    .map(|form| form.id)
                    == Some(crate::parse::BuiltinShapeId::LetValue)
            })
}
