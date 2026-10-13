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
//! right-hand side it is, else a leaf no binder names. A leaf read inside a body the load reads
//! through is keyed on that body's captures too, since the body runs once per what it captures.
//!
//! Trees on the chain are memoized per shape and site; a binder already being read reads as a leaf,
//! so no tree is built through itself.

use crate::memory::{BumpVec, collect, resident};
use crate::parse::{ExpressionPart, KLiteral};
use crate::scope::{
    Binder, BodyShape, CaptureSlot, CaptureSource, Coordinate, KnotNode, Leaf, NodeCapture,
    Position, ShapeKind, Site, Slot, Static, StaticSolution, Target, Tree, Within,
};
use crate::symbols::{BinderSymbol, ValueSymbol};
use crate::type_lattice::KType;
use crate::values::digest::{Operator, literal_digest, type_digest};

use super::super::evaluate::{Form, of_part};
use super::{Landing, Pass};
use crate::knot::module::view::Ascription;

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
    ) -> Tree<'graph> {
        self.tree_in(Scope::Chain(level), part, None)
    }

    /// The tree of `part` read in `scope`, where `named` is the `LET` binder whose whole right-hand
    /// side it is.
    fn tree_in(
        &mut self,
        scope: Scope<'graph>,
        part: &'graph ExpressionPart<'graph>,
        named: Option<Slot>,
    ) -> Tree<'graph> {
        let shape = self.scope_shape(scope);
        let site = Site::of(part);
        if let Scope::Chain(_) = scope
            && let Some((_, _, tree)) =
                (self.trees.iter()).find(|(at, part, _)| std::ptr::eq(*at, shape) && *part == site)
        {
            return *tree;
        }
        let fallback = |pass: &Self| match named {
            Some(slot) => pass.leaf_on(scope, slot, &[]),
            None => Tree::Leaf(Leaf::Unnamed(site)),
        };
        let tree = match of_part(shape, part) {
            Form::Name(name) => match shape.mention(Site::of(name)) {
                Some(mention) => self.resolve(scope, mention.coordinate, site),
                None => fallback(self),
            },
            Form::Member {
                operand,
                label: BinderSymbol::Value(label),
                ..
            } => {
                let through = self.tree_in(scope, operand, None);
                self.read_member(through, label, site)
                    .unwrap_or_else(|| fallback(self))
            }
            Form::Ascribe(node, mode) => {
                let [operand, _, ascribed] = node.parts else {
                    unreachable!("an ascription has an operand, its keyword and a type")
                };
                match shape.typed_expression(Site::of(&ascribed.value)) {
                    Static::Closed(application) => {
                        let source = self.tree_in(scope, &operand.value, None);
                        Tree::View {
                            operator: match mode {
                                Ascription::Opaque => Operator::Opaque,
                                Ascription::Transparent => Operator::Transparent,
                            },
                            application,
                            source: resident(self.writer, source),
                        }
                    }
                    _ => fallback(self),
                }
            }
            Form::Leaf(ExpressionPart::Literal(literal)) => match literal {
                KLiteral::Number(_)
                | KLiteral::String(_)
                | KLiteral::Boolean(_)
                | KLiteral::Null => Tree::Digest(literal_digest(literal)),
            },
            Form::Leaf(ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_)) => {
                match shape.typed_expression(site) {
                    Static::Closed(handle) => Tree::Digest(type_digest(handle)),
                    _ => fallback(self),
                }
            }
            Form::Lambda(node) => match Site::of_body(node).and_then(|body| shape.nested(body)) {
                Some(body) => self
                    .knot(scope, &[(body, false)], 0, site)
                    .unwrap_or_else(|| fallback(self)),
                None => fallback(self),
            },
            _ => fallback(self),
        };
        if let Scope::Chain(_) = scope {
            self.trees.push((shape, site, tree));
        }
        tree
    }

    /// The tree of what `coordinate` holds, read in `scope` at the part at `site`.
    fn resolve(
        &mut self,
        scope: Scope<'graph>,
        coordinate: Coordinate,
        site: Site,
    ) -> Tree<'graph> {
        let unnamed = Tree::Leaf(Leaf::Unnamed(site));
        match scope {
            Scope::Chain(level) => match self.follow(level, coordinate).landing {
                Landing::Slot { level, slot } => self.slot_tree(Scope::Chain(level), slot, site),
                Landing::Builtin(_) | Landing::Open { .. } => unnamed,
            },
            Scope::Body { shape, captures } => match coordinate {
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(slot),
                } => self.slot_tree(scope, slot, site),
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Capture(capture),
                } => match shape.top_capture(capture) {
                    Some(top) => self.slot_tree(Scope::Chain(0), top, site),
                    None => (captures.iter())
                        .find(|(at, _)| *at as usize == capture.index())
                        .map_or(unnamed, |(_, tree)| *tree),
                },
                _ => unnamed,
            },
        }
    }

    /// The tree of what the binder at `slot` of `scope`'s shape holds, read at the part at `site`.
    fn slot_tree(&mut self, scope: Scope<'graph>, slot: Slot, site: Site) -> Tree<'graph> {
        let shape = self.scope_shape(scope);
        let key = (std::ptr::from_ref(shape).addr(), slot);
        if self.reading.contains(&key) {
            return self.leaf_on(scope, slot, &[]);
        }
        self.reading.push(key);
        let tree = self.binder_tree(scope, slot, site);
        self.reading.pop();
        tree
    }

    fn binder_tree(&mut self, scope: Scope<'graph>, slot: Slot, site: Site) -> Tree<'graph> {
        let shape = self.scope_shape(scope);
        if let BinderSymbol::Type(_) = shape.slot_name(slot) {
            return match shape.declared_type(slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Tree::Digest(type_digest(handle))
                }
                _ => self.leaf_on(scope, slot, &[]),
            };
        }
        if let Some(body) = shape.births(slot) {
            if body.kind() == ShapeKind::Module {
                let captures = self.captures_of(scope, body, site);
                return Tree::Body {
                    shape: body,
                    captures,
                };
            }
            let component = shape.component_of(slot);
            let mut bodies = BumpVec::with_capacity_in(component.members.len(), self.scratch);
            for member in component.members {
                match shape.births(*member) {
                    Some(body) if body.kind() == ShapeKind::Callable => {
                        bodies.push((body, keeps_scheme(shape, *member)));
                    }
                    _ => return self.leaf_on(scope, slot, &[]),
                }
            }
            let index = (component.members.iter())
                .position(|member| *member == slot)
                .expect("a binder is a member of its component");
            return self
                .knot(scope, &bodies, index as u32, site)
                .unwrap_or_else(|| self.leaf_on(scope, slot, &[]));
        }
        if shape.position_of(slot) == Position::PARAMETER {
            return self.leaf_on(scope, slot, &[]);
        }
        match shape.rhs(slot) {
            Some(rhs) => self.tree_in(scope, rhs, Some(slot)),
            None => self.leaf_on(scope, slot, &[]),
        }
    }

    /// A leaf on the binder at `slot` of `scope`'s shape, read through `chain`: keyed on the
    /// captures of a body read through.
    fn leaf_on(&self, scope: Scope<'graph>, slot: Slot, chain: &[ValueSymbol]) -> Tree<'graph> {
        let within = match scope {
            Scope::Body { captures, .. } if !captures.is_empty() => {
                Some(&*resident(self.writer, Within::Body(captures)))
            }
            _ => None,
        };
        Tree::Leaf(Leaf::Bound {
            binder: Binder {
                shape: self.scope_shape(scope),
                slot,
            },
            chain: collect(self.writer, chain.iter().copied()),
            within,
        })
    }

    /// `tree`'s value member `label`: through a body, the member's binding there; through a leaf,
    /// the leaf with its chain extended. `None` through anything else.
    fn read_member(
        &mut self,
        tree: Tree<'graph>,
        label: ValueSymbol,
        site: Site,
    ) -> Option<Tree<'graph>> {
        match tree {
            Tree::Body { shape, captures } => {
                let (slot, _) = shape.slot(BinderSymbol::Value(label))?;
                Some(self.slot_tree(Scope::Body { shape, captures }, slot, site))
            }
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
        }
    }

    /// The trees of what `body`, born in `scope` at the part at `site`, composes: each capture its
    /// content counts, beside its closure slot — a builder capture's source, and a type capture's.
    fn captures_of(
        &mut self,
        scope: Scope<'graph>,
        body: &'graph BodyShape<'graph>,
        site: Site,
    ) -> &'graph [(u32, Tree<'graph>)] {
        let mut captures = BumpVec::with_capacity_in(body.capture_count(), self.scratch);
        for index in 0..body.capture_count() {
            let capture = CaptureSlot(index as u32);
            if !body.composes(capture) {
                continue;
            }
            let tree = match source_of_capture(body, capture) {
                CaptureSource::Read(coordinate) => self.resolve(scope, coordinate, site),
                _ => Tree::Leaf(Leaf::Unnamed(site)),
            };
            captures.push((index as u32, tree));
        }
        collect(self.writer, captures.iter().copied())
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
    ) -> Option<Tree<'graph>> {
        let mut members = BumpVec::with_capacity_in(bodies.len(), self.scratch);
        for (body, keeps_scheme) in bodies.iter().copied() {
            let instance = born_at(body, keeps_scheme)?;
            let mut captures = BumpVec::with_capacity_in(body.capture_count(), self.scratch);
            for slot in 0..body.capture_count() {
                let capture = CaptureSlot(slot as u32);
                if !body.composes(capture) {
                    continue;
                }
                let held = match source_of_capture(body, capture) {
                    CaptureSource::Member { index, .. } => NodeCapture::Fellow(index),
                    CaptureSource::Read(coordinate) => {
                        NodeCapture::Tree(self.resolve(scope, coordinate, site))
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
        Some(Tree::Knot {
            members: collect(self.writer, members.iter().copied()),
            index,
        })
    }
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
