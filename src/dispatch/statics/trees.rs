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

use crate::memory::{BumpVec, collect, resident};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    Binder, BodyShape, CaptureSlot, CaptureSource, Coordinate, KnotNode, Leaf, Narrowing,
    NodeCapture, ParameterBinding, Position, ShapeError, ShapeKind, Site, Slot, Static,
    StaticSolution, Target, Tree, Unfolded, Within,
};
use crate::symbols::{BinderSymbol, ValueSymbol};
use crate::type_lattice::KType;
use crate::values::digest::{Operator, literal_digest, type_digest};

use super::super::evaluate::{Form, Wanted, of_node, of_part, slots};
use super::{Landing, Pass, done};
use crate::knot::module::view::Ascription;

type Read<'graph, T> = Result<T, ShapeError<'graph>>;

/// Where a tree is read: a shape on the chain, or a `MODULE` body read through its tree.
#[derive(Clone, Copy)]
pub(super) enum Scope<'graph> {
    Chain(usize),
    Body {
        shape: &'graph BodyShape<'graph>,
        captures: &'graph [(u32, Tree<'graph>)],
    },
}

impl<'p, 'graph: 'p> Pass<'p, '_, 'graph> {
    /// The shape `scope` reads in.
    fn scope_shape(&self, scope: Scope<'graph>) -> &'graph BodyShape<'graph> {
        match scope {
            Scope::Chain(level) => self.chain[level].shape,
            Scope::Body { shape, .. } => shape,
        }
    }

    /// The tree of `part`, read in the shape at chain level `level`.
    pub(super) fn tree(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Read<'graph, Tree<'graph>> {
        self.tree_in(Scope::Chain(level), part, None)
    }

    /// The summary of the callable body at chain level `level`: the tree of its last statement.
    pub(super) fn summarize(&mut self, level: usize) -> Read<'graph, Option<Tree<'graph>>> {
        let shape = self.chain[level].shape;
        let Some(last) = shape.body().len().checked_sub(1) else {
            return Ok(None);
        };
        let scope = Scope::Chain(level);
        // A binding statement's value is what it binds.
        let bound = (0..shape.slots())
            .map(|slot| Slot(slot as u32))
            .find(|slot| {
                shape.position_of(*slot).statement_index() == Some(last)
                    && !matches!(shape.slot_name(*slot), BinderSymbol::Type(_))
                    && shape.registration(*slot).is_none()
            });
        let node = &shape.body()[last];
        let tree = match bound {
            Some(slot) => self.slot_tree(scope, slot, Site::of_node(node))?,
            None => self.tree_form(scope, of_node(shape, node), Site::of_node(node), None)?,
        };
        Ok(Some(tree))
    }

    /// The tree of `part` read in `scope`, where `named` is the `LET` binder whose whole right-hand
    /// side it is.
    fn tree_in(
        &mut self,
        scope: Scope<'graph>,
        part: &'graph ExpressionPart<'graph>,
        named: Option<Slot>,
    ) -> Read<'graph, Tree<'graph>> {
        let shape = self.scope_shape(scope);
        self.tree_form(scope, of_part(shape, part), Site::of(part), named)
    }

    /// The tree of what the evaluator reads as `form`, written at `site` in `scope`.
    fn tree_form(
        &mut self,
        scope: Scope<'graph>,
        form: Form<'graph>,
        site: Site,
        named: Option<Slot>,
    ) -> Read<'graph, Tree<'graph>> {
        let shape = self.scope_shape(scope);
        if let Scope::Chain(_) = scope
            && let Some((_, _, tree)) =
                (self.trees.iter()).find(|(at, part, _)| std::ptr::eq(*at, shape) && *part == site)
        {
            return Ok(*tree);
        }
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
                match shape.typed_expression(Site::of(&ascribed.value)) {
                    Static::Closed(application) => {
                        let source = self.tree_in(scope, &operand.value, None)?;
                        Tree::View {
                            operator: match mode {
                                Ascription::Opaque => Operator::Opaque,
                                Ascription::Transparent => Operator::Transparent,
                            },
                            application,
                            source: resident(self.writer, source),
                        }
                    }
                    _ => self.fallback(scope, named, site)?,
                }
            }
            Form::Leaf(ExpressionPart::Literal(literal)) => Tree::Digest(literal_digest(literal)),
            Form::Leaf(ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_)) => {
                match shape.typed_expression(site) {
                    Static::Closed(handle) => Tree::Digest(type_digest(handle)),
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
        if let Scope::Chain(_) = scope {
            self.trees.push((shape, site, tree));
        }
        Ok(tree)
    }

    /// The tree of what `coordinate` holds, read in `scope` at the part at `site`.
    fn resolve(
        &mut self,
        scope: Scope<'graph>,
        coordinate: Coordinate,
        site: Site,
    ) -> Read<'graph, Tree<'graph>> {
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
    fn slot_tree(
        &mut self,
        scope: Scope<'graph>,
        slot: Slot,
        site: Site,
    ) -> Read<'graph, Tree<'graph>> {
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
        scope: Scope<'graph>,
        slot: Slot,
        site: Site,
    ) -> Read<'graph, Tree<'graph>> {
        let shape = self.scope_shape(scope);
        if let BinderSymbol::Type(_) = shape.slot_name(slot) {
            return match shape.declared_type(slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Ok(Tree::Digest(type_digest(handle)))
                }
                _ => self.leaf_on(scope, slot),
            };
        }
        if let Some(body) = shape.births(slot) {
            if body.kind() == ShapeKind::Module {
                // A body not yet typed has not laid its type captures down.
                if !done(body) {
                    return self.leaf_on(scope, slot);
                }
                let captures = self.captures_of(scope, body, site)?;
                return Ok(Tree::Body {
                    shape: body,
                    captures,
                });
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
        match shape.rhs(slot) {
            Some(rhs) => self.tree_in(scope, rhs, Some(slot)),
            None => self.leaf_on(scope, slot),
        }
    }

    /// The tree an expression the load cannot see reads as: a leaf on the `LET` binder `named`
    /// whose whole right-hand side it is, else a leaf no binder names, keyed on `site`.
    fn fallback(
        &mut self,
        scope: Scope<'graph>,
        named: Option<Slot>,
        site: Site,
    ) -> Read<'graph, Tree<'graph>> {
        match named {
            Some(slot) => self.leaf_on(scope, slot),
            None => Ok(Tree::Leaf(Leaf::Unnamed(site))),
        }
    }

    /// `tree`, or where there is none, the [fallback](Self::fallback).
    fn or_fallback(
        &mut self,
        tree: Option<Tree<'graph>>,
        scope: Scope<'graph>,
        named: Option<Slot>,
        site: Site,
    ) -> Read<'graph, Tree<'graph>> {
        match tree {
            Some(tree) => Ok(tree),
            None => self.fallback(scope, named, site),
        }
    }

    /// A leaf on the binder at `slot` of `scope`'s shape. A `MODULE` or `GROUP` body's binder holds
    /// one value per run of the body, and the body's captures say which run, so its leaf is keyed
    /// on them wherever it is read: on the chain, as the shape enclosing the body resolves them,
    /// and through a body's tree, as the tree holds them.
    fn leaf_on(&mut self, scope: Scope<'graph>, slot: Slot) -> Read<'graph, Tree<'graph>> {
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
                collect(self.writer, key.iter().copied())
            }
            // A body with no statement reads nothing, so captures nothing.
            Scope::Chain(level) => match shape.body().first() {
                Some(first) => {
                    self.builder_captures(Scope::Chain(level - 1), shape, Site::of_node(first))?
                }
                None => &[][..],
            },
        };
        Ok(Tree::Leaf(Leaf::Bound {
            binder: Binder { shape, slot },
            chain: &[],
            within: (!key.is_empty()).then(|| resident(self.writer, Within::Body(key))),
        }))
    }

    /// `tree`'s value member `label`, read at the part at `site`: through a body, the member's
    /// binding there; through a leaf, the leaf with its chain extended. `None` through anything
    /// else.
    fn read_member(
        &mut self,
        tree: Tree<'graph>,
        label: ValueSymbol,
        site: Site,
    ) -> Read<'graph, Option<Tree<'graph>>> {
        Ok(match tree {
            Tree::Body { shape, captures } => match shape.slot(BinderSymbol::Value(label)) {
                Some((slot, _)) => {
                    Some(self.slot_tree(Scope::Body { shape, captures }, slot, site)?)
                }
                None => None,
            },
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
            }) => {
                let mut extended = BumpVec::with_capacity_in(chain.len() + 1, self.scratch);
                extended.extend(chain.iter().copied());
                extended.push(label);
                Some(Tree::Leaf(Leaf::Bound {
                    binder,
                    chain: collect(self.writer, extended.iter().copied()),
                    within,
                }))
            }
            Tree::Leaf(Leaf::Unnamed(site)) => Some(Tree::Leaf(Leaf::Unnamed(site))),
            Tree::Digest(_) | Tree::Knot { .. } | Tree::View { .. } => None,
        })
    }

    /// The trees of what `body`, born in `scope` at the part at `site`, composes: each capture its
    /// content counts, beside its closure slot — a builder capture's source, and a type capture's.
    fn captures_of(
        &mut self,
        scope: Scope<'graph>,
        body: &'graph BodyShape<'graph>,
        site: Site,
    ) -> Read<'graph, &'graph [(u32, Tree<'graph>)]> {
        self.composed_captures(scope, body, body.capture_count(), site)
    }

    /// [`captures_of`](Self::captures_of) short of the type captures, which a module body's `OVER`
    /// list names as builder captures too: what says which run of the body a binder's value is
    /// from, known before the body is typed.
    fn builder_captures(
        &mut self,
        scope: Scope<'graph>,
        body: &'graph BodyShape<'graph>,
        site: Site,
    ) -> Read<'graph, &'graph [(u32, Tree<'graph>)]> {
        self.composed_captures(scope, body, body.captures().len(), site)
    }

    /// The trees of the first `count` closure slots of `body` its content composes.
    fn composed_captures(
        &mut self,
        scope: Scope<'graph>,
        body: &'graph BodyShape<'graph>,
        count: usize,
        site: Site,
    ) -> Read<'graph, &'graph [(u32, Tree<'graph>)]> {
        let mut captures = BumpVec::with_capacity_in(count, self.scratch);
        for index in 0..count {
            let capture = CaptureSlot(index as u32);
            if !body.composes(capture) {
                continue;
            }
            let tree = match source_of_capture(body, capture) {
                CaptureSource::Read(coordinate) => self.resolve(scope, coordinate, site)?,
                _ => Tree::Leaf(Leaf::Unnamed(site)),
            };
            captures.push((index as u32, tree));
        }
        Ok(collect(self.writer, captures.iter().copied()))
    }

    /// The knot of closures `bodies` are born as in `scope` at the part at `site`, beside member
    /// `index`; each body beside whether its binder keeps its scheme. `None` where the load cannot
    /// say what solution one is born at.
    fn knot(
        &mut self,
        scope: Scope<'graph>,
        bodies: &[(&'graph BodyShape<'graph>, bool)],
        index: u32,
        site: Site,
    ) -> Read<'graph, Option<Tree<'graph>>> {
        let mut members = BumpVec::with_capacity_in(bodies.len(), self.scratch);
        for (body, keeps_scheme) in bodies.iter().copied() {
            // A closure's type captures are laid down once its body is typed.
            if !self.typed_on_demand(body)? {
                return Ok(None);
            }
            let Some(instance) = born_at(body, keeps_scheme) else {
                return Ok(None);
            };
            let mut captures = BumpVec::with_capacity_in(body.capture_count(), self.scratch);
            for slot in 0..body.capture_count() {
                let capture = CaptureSlot(slot as u32);
                if !body.composes(capture) {
                    continue;
                }
                let held = match source_of_capture(body, capture) {
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
                shape: body,
                instance,
                captures: collect(self.writer, captures.iter().copied()),
            });
        }
        Ok(Some(Tree::Knot {
            members: collect(self.writer, members.iter().copied()),
            index,
        }))
    }

    /// The tree of the keyworded use `node` of the shape at `level`, unfolded where the pass
    /// selected one registration there whose solve is closed: `None` where it does not unfold.
    fn keyworded(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Read<'graph, Option<Tree<'graph>>> {
        let shape = self.chain[level].shape;
        let Some(index) = shape.candidate_lists().position(Site::of_node(node)) else {
            return Ok(None);
        };
        let Narrowing::Selected(coordinate) = self.chain[level].uses[index].narrowing else {
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
        let Static::Closed(registered) = declaring.registered_type(slot) else {
            return Ok(None);
        };
        let ParameterBinding::Named(names) = registered.parameters else {
            return Ok(None);
        };
        // Each `FOR ALL` variable at the closed point the pass solved it to at this use.
        let mut solution = BumpVec::new_in(self.scratch);
        if !registered.quantifier_map.is_empty() {
            let Some((_, points)) =
                (self.selections.iter().rev()).find(|(at, _)| *at == Site::of_node(node))
            else {
                return Ok(None);
            };
            let points = *points;
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
    ) -> Read<'graph, Option<Tree<'graph>>> {
        let [head, argument] = node.parts else {
            return Ok(None);
        };
        let ExpressionPart::RecordLiteral(fields) = &argument.value else {
            return Ok(None);
        };
        let Tree::Knot { members, index } = self.tree(level, &head.value)? else {
            return Ok(None);
        };
        let callee = members[index as usize].shape;
        if !matches!(callee.callable_type(), Static::Closed(callable) if callable.ktype.as_type().is_some())
        {
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
        arguments: &[(Slot, Tree<'graph>)],
        solution: &[(Slot, KType)],
        site: Site,
    ) -> Read<'graph, Option<Tree<'graph>>> {
        let Some(summary) = self.summary(callee)? else {
            return Ok(None);
        };
        let call = Unfolded {
            callee,
            arguments: collect(self.writer, arguments.iter().copied()),
            solution: collect(self.writer, solution.iter().copied()),
        };
        self.substituted(summary, &call, site).map(Some)
    }

    /// The summary of `callee`, typing it first where the walk has not: `None` where it is
    /// recursive, or where the pass is typing unfilled code.
    fn summary(&mut self, callee: &'graph BodyShape<'graph>) -> Read<'graph, Option<Tree<'graph>>> {
        if self.unfilled {
            return Ok(None);
        }
        if let Some(at) = (self.typing.iter()).position(|typing| std::ptr::eq(*typing, callee)) {
            for typing in self.typing[at..].iter() {
                if !self
                    .recursive
                    .iter()
                    .any(|known| std::ptr::eq(*known, *typing))
                {
                    self.recursive.push(*typing);
                }
            }
            return Ok(None);
        }
        if let Some((_, summary)) =
            (self.summaries.iter()).find(|(at, _)| std::ptr::eq(*at, callee))
        {
            return Ok(*summary);
        }
        if done(callee) || !self.typed_on_demand(callee)? {
            return Ok(None);
        }
        Ok((self.summaries.iter())
            .find(|(at, _)| std::ptr::eq(*at, callee))
            .and_then(|(_, summary)| *summary))
    }

    /// Whether the callable body `body` is typed, typing it now where the walk has not reached it:
    /// `false` where it is being typed, or the chain does not hold the shape it is nested in.
    fn typed_on_demand(&mut self, body: &'graph BodyShape<'graph>) -> Read<'graph, bool> {
        if done(body) {
            return Ok(true);
        }
        if self.unfilled || (self.typing.iter()).any(|typing| std::ptr::eq(*typing, body)) {
            return Ok(false);
        }
        // The shape `body` is nested in is on the chain, since a read reaches only definitions in
        // scope; the chain above it is set aside while the body is typed.
        let Some(parent) = (0..self.chain.len()).rev().find(|level| {
            (self.chain[*level].shape.nested_shapes().iter())
                .any(|(_, nested)| std::ptr::eq(*nested, body))
        }) else {
            return Ok(false);
        };
        let mut stash = BumpVec::with_capacity_in(self.chain.len() - parent - 1, self.scratch);
        stash.extend(self.chain.drain(parent + 1..));
        let typed = self.nest(parent, body);
        self.chain.extend(stash);
        typed?;
        Ok(done(body))
    }

    /// `tree`, a summary of `call`'s callee, with the call substituted: each parameter's leaf by
    /// its argument's tree read through the leaf's chain, each `FOR ALL` variable's by its point,
    /// and every other leaf of the callee's body keyed on the call.
    fn substituted(
        &mut self,
        tree: Tree<'graph>,
        call: &Unfolded<'graph>,
        site: Site,
    ) -> Read<'graph, Tree<'graph>> {
        Ok(match tree {
            Tree::Digest(_) | Tree::Leaf(Leaf::Unnamed(_)) => tree,
            Tree::Body { shape, captures } => Tree::Body {
                shape,
                captures: self.substituted_captures(captures, call, site)?,
            },
            Tree::Knot { members, index } => {
                let mut nodes = BumpVec::with_capacity_in(members.len(), self.scratch);
                for node in members {
                    let mut captures = BumpVec::with_capacity_in(node.captures.len(), self.scratch);
                    for (slot, capture) in node.captures {
                        captures.push((
                            *slot,
                            match capture {
                                NodeCapture::Fellow(fellow) => NodeCapture::Fellow(*fellow),
                                NodeCapture::Tree(tree) => {
                                    NodeCapture::Tree(self.substituted(*tree, call, site)?)
                                }
                            },
                        ));
                    }
                    nodes.push(KnotNode {
                        captures: collect(self.writer, captures.iter().copied()),
                        ..*node
                    });
                }
                Tree::Knot {
                    members: collect(self.writer, nodes.iter().copied()),
                    index,
                }
            }
            Tree::View {
                operator,
                application,
                source,
            } => {
                let source = self.substituted(*source, call, site)?;
                Tree::View {
                    operator,
                    application,
                    source: resident(self.writer, source),
                }
            }
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
            }) => {
                if std::ptr::eq(binder.shape, call.callee) {
                    if let Some((_, argument)) =
                        (call.arguments.iter()).find(|(slot, _)| *slot == binder.slot)
                    {
                        let mut read = Some(*argument);
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
                    if let Some((_, point)) =
                        (call.solution.iter()).find(|(slot, _)| *slot == binder.slot)
                        && chain.is_empty()
                    {
                        return Ok(Tree::Digest(type_digest(*point)));
                    }
                }
                let within = match within {
                    Some(Within::Body(captures)) => Some(Within::Body(
                        self.substituted_captures(captures, call, site)?,
                    )),
                    Some(Within::Call(inner)) => {
                        let mut arguments =
                            BumpVec::with_capacity_in(inner.arguments.len(), self.scratch);
                        for (slot, tree) in inner.arguments {
                            arguments.push((*slot, self.substituted(*tree, call, site)?));
                        }
                        Some(Within::Call(Unfolded {
                            arguments: collect(self.writer, arguments.iter().copied()),
                            ..*inner
                        }))
                    }
                    // A module body's binder is keyed on its captures, which read nothing.
                    None if binder.shape.kind() == ShapeKind::Module => None,
                    None if inside(binder.shape, call.callee) => Some(Within::Call(*call)),
                    None => None,
                };
                Tree::Leaf(Leaf::Bound {
                    binder,
                    chain,
                    within: within.map(|within| resident(self.writer, within)),
                })
            }
        })
    }

    fn substituted_captures(
        &mut self,
        captures: &'graph [(u32, Tree<'graph>)],
        call: &Unfolded<'graph>,
        site: Site,
    ) -> Read<'graph, &'graph [(u32, Tree<'graph>)]> {
        let mut substituted = BumpVec::with_capacity_in(captures.len(), self.scratch);
        for (slot, tree) in captures {
            substituted.push((*slot, self.substituted(*tree, call, site)?));
        }
        Ok(collect(self.writer, substituted.iter().copied()))
    }
}

/// Whether `shape` is `callee` or a body nested in it, at any depth: a binder bound once per call
/// of `callee`.
fn inside<'graph>(shape: &BodyShape<'graph>, callee: &BodyShape<'graph>) -> bool {
    std::ptr::eq(shape, callee)
        || (callee.nested_shapes().iter()).any(|(_, nested)| inside(shape, nested))
        || (0..callee.slots()).any(|slot| {
            callee
                .births(Slot(slot as u32))
                .is_some_and(|born| inside(shape, born))
        })
}

/// Where capture `capture` of `body` is read from at birth: a builder capture's source, or a type
/// capture's coordinate.
fn source_of_capture(body: &BodyShape<'_>, capture: CaptureSlot) -> CaptureSource {
    let builder = body.captures();
    match builder.get(capture.index()) {
        Some(spec) => spec.source,
        None => CaptureSource::Read(body.type_captures()[capture.index() - builder.len()]),
    }
}

/// The solution the closure `body` is born at: its born instance where the load fixed one closed,
/// and nothing where it is born unquantified or, its binder keeping its scheme, quantified. `None`
/// where the load cannot say.
fn born_at<'graph>(body: &'graph BodyShape<'graph>, keeps_scheme: bool) -> Option<&'graph [KType]> {
    match body.born_instance() {
        Some(StaticSolution::Closed(solution)) => Some(solution),
        Some(StaticSolution::Rigid { .. }) => None,
        None => match body.callable_type() {
            Static::Closed(callable) if callable.ktype.as_type().is_some() => Some(&[]),
            Static::Rigid { value, .. } if value.ktype.as_type().is_some() => Some(&[]),
            Static::Closed(_) | Static::Rigid { .. } if keeps_scheme => Some(&[]),
            // A scheme born with no instance is not typed yet, or never typed.
            _ => None,
        },
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
