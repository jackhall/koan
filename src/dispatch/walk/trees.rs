//! The walk's [content trees](crate::scope::Tree): what the load knows of each
//! module-valued expression's content, read off the program's text as the
//! [module design](../../../design/modules.md#the-load-and-the-run) tables it.
//!
//! A tree is read in a **scope**: a shape on the pass's chain, whose captures the chain's own
//! [`follow`](super::Pass::follow) walk resolves, or a `MODULE` body the load reads a member
//! through, whose captures are its tree's and whose top-level reads are the program's. A name is
//! the tree of what its binder holds — a body, a knot of closures, a `LET`'s right-hand side — and
//! a parameter a leaf on its binder; a type name written over names is the type value it names,
//! which a binding closes and a call writes its points into. A member read goes through a body to
//! the member's binding and through a leaf by extending its chain. Anything else is a leaf on the
//! `LET` whose whole right-hand side it is, else a leaf no binder names. A leaf on a `MODULE` or
//! `GROUP` body's binder is keyed on the body's builder captures too, wherever it is read, since
//! they say which run of the body its value is from.
//!
//! A call on the chain **unfolds** ([design](../../../design/modules.md#a-call-unfolds)): a
//! keyworded use the pass selected one registration at, its type closed or written over the run's
//! variables, or a call by name whose head's tree is a closure, of an unquantified callee, one
//! whose every `FOR ALL` variable solves to a closed point, or one whose summary reads nothing its
//! call solves. Its tree is the callee's **summary** — the tree of its body's last statement, its
//! parameters leaves — with each parameter's argument tree, retyped to the parameter's declared
//! type, and each variable's point substituted, and every other leaf of the callee's keyed on the
//! call. A tree's types may be written over names, which a binding writes into as it substitutes
//! the tree's leaves. The walk types a callee before the first call it unfolds, on demand where it
//! has not reached it yet; a call unfolds only into a callee whose strongly connected component
//! has closed and is trivial ([recursion](../../build/README.md#recursion)).
//!
//! Writing a call or a binding into a tree, and building the retypes, views and member reads
//! over it, is [tree application](super::super::graph::trees): the walk applies over its own facts,
//! the records it is writing, binding nothing ([`TreeFacts`]).
//!
//! A keyworded call only an answer selects among is a **call leaf**, keyed on its candidates'
//! callees and its argument trees, so two writes of one call are one tree at load and under every
//! binding; the load's rigid selection never unfolds it. A binding substituting the leaf answers the
//! use's selection closed ([`Selection::selected`](crate::scope::Selection::selected)) and
//! unfolds the candidate it selects, as a fresh load does; where none
//! is selected, the run binds the `LET` holding the call at its statement.
//!
//! At each call the load's walk resolves to one callable body — a keyworded use it selected, or
//! a call by name whose head's tree is a closure — it records what the call passes at each of the
//! callee's slots ([`Traced`]): load content, or a `FOR ALL` name or parameter of the callable
//! owning the call passed on bare, a parameter only at its callee's own closed declared type.
//! The [binding graph's fold](../../build/README.md#the-binding-graphs-text) reads those records as
//! its calls' edges.
//!
//! Trees on the chain are memoized per shape and site, and each binder's tree once on the chain and
//! once through each body's tree, except where a cycle cut on an outer binder, a fallback the
//! walk's progress could still sharpen, or a leaf keyed on the read site shaped it. A binder
//! already being read reads as a leaf, so no tree is built through itself.

use crate::elaborate::judge::{self, NoMarks};
use crate::memory::{BumpVec, collect, resident};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    Binder, BinderAt, BodyShape, Called, Callee, Candidate, CaptureSlot, CaptureSource, Content,
    Coordinate, Fixed, Identity, KnotNode, Leaf, Narrowing, NodeCapture, ParameterBinding, Passing,
    Point, Position, Revealed, Segment, Selection, ShapeError, ShapeGroupMap, ShapeKind, Site,
    Slot, Static, StaticCallable, StaticInstance, StaticType, Store, Target, Traced, Tree, Within,
    part_of,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, Interval, KKind, KType, Parametric, TypeNode};
use crate::values::digest::literal_digest;

use super::super::evaluate::{Form, Wanted, of_node, of_part, slots};
use super::super::graph::trees::{
    self as graph, Apply, Fit, Note, TreeFacts, named_type, type_tree,
};
use super::{Landing, Pass, address};
use crate::knot::module::view::Ascription;
use crate::knot::module::view::Exposure;

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

/// What shaped the binder tree being built besides the program's text: the lowest index on the
/// pass's `reading` stack a cycle cut reached, and whether an inner binder's tree standing in it
/// was [provisional](Built::Provisional). A tree is memoized only where neither reaches outside
/// its own build.
#[derive(Clone, Copy)]
pub(super) struct Shaped {
    cut: usize,
    provisional: bool,
}

impl Shaped {
    /// A build no cut or fallback shaped.
    pub(super) const NONE: Shaped = Shaped {
        cut: usize::MAX,
        provisional: false,
    };
}

/// What a `:|`'s fit solved over its underlying, where the load knows it, beside the type its
/// source carries, the member it shows that its source hides, and the unit that read them.
pub(super) type Fitted = (Option<KType>, Option<KType>, Option<Revealed>, super::Tag);

/// A binder's tree as [`binder_tree`](Pass::binder_tree) builds it: settled, or a fallback the
/// walk's progress could still sharpen, which each return names.
enum Built<'o> {
    Settled(Tree<'o>),
    Provisional(Tree<'o>),
}

/// The value slot `shape`'s last statement binds, where it binds one: a binding statement's value
/// is what it binds. Read among the slots of the unit running that statement.
pub(super) fn last_binder(shape: &BodyShape<'_>) -> Option<Slot> {
    let last = shape.body().len().checked_sub(1)?;
    let unit = shape.holds(shape.unit_of_statement(last));
    unit.slots.iter().copied().find(|slot| {
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

    /// A chain level whose chain `scope` reads its paths in.
    fn scope_level(&self, scope: Scope<'o, 'graph>) -> usize {
        match scope {
            Scope::Chain(level) => level,
            Scope::Body { shape, .. } => self.body_level(shape),
        }
    }

    /// A chain level whose chain the body `shape` reads its paths in: its own chain's root where
    /// that chain is being typed — the program's always is; a quote's code's while a tree read
    /// inside it names a body the code captured — else the chain being typed.
    pub(super) fn body_level(&self, shape: &BodyShape<'graph>) -> usize {
        let root = self.shape_at(self.shapes.chain_root(shape.segment()));
        (self.chain.iter())
            .rposition(|level| level.roots_chain && std::ptr::eq(level.shape, root))
            .unwrap_or(self.chain.len() - 1)
    }

    /// The callable cell of the body `body`, read through its own chain's path resolutions.
    pub(super) fn callable_of(&self, body: &'graph BodyShape<'graph>) -> StaticCallable<'o> {
        self.resolved_callable(self.callable_cell(body))
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
            Some(slot) => self.slot_tree(scope, slot, shape.site_of_node(node))?,
            None => {
                let written = Written::Node(shape.site_of_node(node));
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
        named: Option<BinderAt>,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        self.tree_form(
            scope,
            of_part(shape, part),
            Written::Part(shape.site(part)),
            named,
        )
    }

    /// The tree of what the evaluator reads as `form`, written as `written` in `scope`.
    pub(super) fn tree_form(
        &mut self,
        scope: Scope<'o, 'graph>,
        form: Form<'graph>,
        written: Written,
        named: Option<BinderAt>,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        if let Scope::Chain(_) = scope
            && let Some((tree, tag)) = self.trees.get(&(address(shape), written)).copied()
        {
            self.gather(tag);
            return Ok(tree);
        }
        let tree = match self.tree_read(scope, form, written, named)? {
            Built::Settled(tree) => tree,
            // Through a body's tree, the binder tree it stands in is then not memoized; on the
            // chain, `trees` freezes it per site already.
            Built::Provisional(tree) => {
                if let Scope::Body { .. } = scope {
                    self.shaped.provisional = true;
                }
                tree
            }
        };
        if let Scope::Chain(_) = scope {
            let tag = self.innermost();
            self.trees.insert((address(shape), written), (tree, tag));
        }
        Ok(tree)
    }

    /// [`tree_form`](Self::tree_form) unmemoized: provisional where it falls back on what the
    /// walk's progress could sharpen — a type not closed yet, a closure not typed, a call not
    /// unfolded. `tree_form` acts on that only through a body's tree.
    fn tree_read(
        &mut self,
        scope: Scope<'o, 'graph>,
        form: Form<'graph>,
        written: Written,
        named: Option<BinderAt>,
    ) -> Read<'o, Built<'o>> {
        use Built::{Provisional, Settled};
        let shape = self.scope_shape(scope);
        let site = written.site();
        Ok(match form {
            Form::Name(name) => Settled(match shape.mention(shape.site(name)) {
                Some(mention) => self.resolve(scope, mention.coordinate, site)?,
                None => self.fallback(scope, named, site)?,
            }),
            Form::Member {
                operand,
                label: BinderSymbol::Value(label),
                ..
            } => {
                let through = self.tree_in(scope, operand, None)?;
                let read = self.applying().read_member(through, label, site)?;
                Settled(self.or_fallback(read, scope, named, site)?)
            }
            Form::Ascribe(node, mode) => {
                let [operand, _, ascribed] = node.parts else {
                    unreachable!("an ascription has an operand, its keyword and a type")
                };
                let typed = self.type_cell(shape, shape.site(&ascribed.value));
                // An application written over names is one a binding closes, as it substitutes.
                let application = match self.resolved_type(typed) {
                    Static::Closed(application) => Some(Parametric::from(application)),
                    Static::Rigid { value, .. } => Some(value),
                    Static::Unknown => None,
                };
                match application {
                    Some(application) => {
                        let source = self.tree_in(scope, &operand.value, None)?;
                        Settled(match mode {
                            Ascription::Opaque => {
                                self.applying().view_tree(application, source, site)?
                            }
                            // `:!` is a retype.
                            Ascription::Transparent => {
                                self.applying().retyped_tree(source, application, site)?
                            }
                        })
                    }
                    None => Provisional(self.fallback(scope, named, site)?),
                }
            }
            Form::Leaf(ExpressionPart::Literal(literal)) => {
                Settled(Tree::Digest(literal_digest(literal), None))
            }
            Form::Leaf(ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_)) => {
                match self.resolved_type(self.type_cell(shape, site)) {
                    Static::Closed(handle) => Settled(named_type(self.types, handle)),
                    Static::Rigid { value, .. } => Settled(type_tree(self.types, value)),
                    Static::Unknown => Provisional(self.fallback(scope, named, site)?),
                }
            }
            Form::Lambda(node) => {
                match shape.site_of_body(node).and_then(|body| shape.nested(body)) {
                    Some(body) => match self.knot(scope, &[(body, false)], 0, site)? {
                        Some(knot) => Settled(knot),
                        None => Provisional(self.fallback(scope, named, site)?),
                    },
                    None => Settled(self.fallback(scope, named, site)?),
                }
            }
            Form::Call(node, _) => match scope {
                Scope::Chain(level) => match self.keyworded(level, node, named)? {
                    Some(unfolded) => Settled(unfolded),
                    None => Provisional(self.fallback(scope, named, site)?),
                },
                Scope::Body { .. } => Settled(self.fallback(scope, named, site)?),
            },
            Form::Apply(node) => match scope {
                Scope::Chain(level) => match self.by_name(level, node)? {
                    Some(unfolded) => Settled(unfolded),
                    None => Provisional(self.fallback(scope, named, site)?),
                },
                Scope::Body { .. } => Settled(self.fallback(scope, named, site)?),
            },
            _ => Settled(self.fallback(scope, named, site)?),
        })
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
    /// It is built once on the chain, and once through each body's tree, except where a cycle cut
    /// on an outer binder, a fallback the walk's progress could sharpen, or a leaf keyed on the
    /// read site shaped it ([`Shaped`]).
    pub(super) fn slot_tree(
        &mut self,
        scope: Scope<'o, 'graph>,
        slot: Slot,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let key = (address(self.scope_shape(scope)), slot);
        // A body read through its tree is read under the tree's captures, by their address.
        let under = match scope {
            Scope::Chain(_) => None,
            Scope::Body { captures, .. } => Some(captures.as_ptr().addr()),
        };
        if let Some((tree, tag)) = self.binders.get(&(key.0, slot, under)).copied() {
            self.gather(tag);
            return Ok(tree);
        }
        if let Some(at) = self.reading.iter().position(|reading| *reading == key) {
            self.shaped.cut = self.shaped.cut.min(at);
            return self.leaf_on(scope, BinderAt::Slot(slot));
        }
        let depth = self.reading.len();
        self.reading.push(key);
        let outer = std::mem::replace(&mut self.shaped, Shaped::NONE);
        let (tree, provisional) = match self.binder_tree(scope, slot, site) {
            Ok(Built::Settled(tree)) => (Ok(tree), self.shaped.provisional),
            Ok(Built::Provisional(tree)) => (Ok(tree), true),
            Err(error) => (Err(error), self.shaped.provisional),
        };
        let cut = self.shaped.cut;
        self.shaped = Shaped {
            cut: outer.cut.min(cut),
            provisional: outer.provisional || provisional,
        };
        self.reading.pop();
        // A cut on this binder itself reads alike wherever the binder is read from.
        if let Ok(tree) = tree
            && !provisional
            && cut >= depth
            && tree.identity() != Identity::Unnamed
        {
            let tag = self.innermost();
            self.binders.insert((key.0, slot, under), (tree, tag));
        }
        tree
    }

    /// The lexical variable the run-bound type name `name` at `slot` of `shape` is, as a written
    /// read of it reads it ([`elaborate`](crate::elaborate)'s classing): its callable's own `FOR
    /// ALL` variable where the load typed the callable, else the variable at its level bounded by
    /// `Any`, which is settled only outside a callable the load has yet to type. `None` where the
    /// name has no level.
    fn name_variable(
        &self,
        shape: &'graph BodyShape<'graph>,
        slot: Slot,
        name: crate::symbols::TypeSymbol,
    ) -> Option<(Parametric, bool)> {
        let level = (self.shapes).level(shape.segment(), crate::scope::Address::Slot(slot))?;
        let mut settled = true;
        if shape.kind() == ShapeKind::Callable {
            let map = match self.callable_of(shape) {
                Static::Closed(callable) => Some(callable.quantifier_map),
                Static::Rigid { value, .. } => Some(value.quantifier_map),
                Static::Unknown => None,
            };
            settled = map.is_some();
            if let Some(index) = map.and_then(|map| map.get(name))
                && let Some(own) =
                    (self.born(shape.segment())).and_then(|born| born.levels.get(index).copied())
            {
                return Some((own, true));
            }
        }
        Some((self.types.lexical(level, name, KType::ANY), settled))
    }

    /// [`slot_tree`](Self::slot_tree) unmemoized.
    fn binder_tree(
        &mut self,
        scope: Scope<'o, 'graph>,
        slot: Slot,
        site: Site,
    ) -> Read<'o, Built<'o>> {
        use Built::{Provisional, Settled};
        let shape = self.scope_shape(scope);
        if let BinderSymbol::Type(name) = shape.slot_name(slot) {
            return match self.declared_cell(shape, slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Ok(Settled(named_type(self.types, handle)))
                }
                Static::Closed(_) => self.leaf_on(scope, BinderAt::Slot(slot)).map(Settled),
                // A type name written over names is the type value it names, which a binding
                // closes as a fresh load reads it.
                Static::Rigid { value, .. } => Ok(Settled(type_tree(self.types, value))),
                // A surfaced name its module's signature declares exactly is that type.
                Static::Unknown
                    if let Some(exactly) = (self.level_of(shape.segment()))
                        .and_then(|level| self.surfaced_type(level, slot)) =>
                {
                    Ok(Settled(named_type(self.types, exactly)))
                }
                Static::Unknown => match self.name_variable(shape, slot, name) {
                    Some((variable, true)) => Ok(Settled(type_tree(self.types, variable))),
                    Some((variable, false)) => Ok(Provisional(type_tree(self.types, variable))),
                    None => self.leaf_on(scope, BinderAt::Slot(slot)).map(Provisional),
                },
            };
        }
        if let Some(body) = shape.births(slot) {
            if body.kind() == ShapeKind::Module {
                // A body not yet typed has not laid its type captures down.
                if !self.done(body) {
                    return self.leaf_on(scope, BinderAt::Slot(slot)).map(Provisional);
                }
                let captures = self.captures_of(scope, body, site)?;
                return Ok(Settled(Tree::body(body, captures)));
            }
            let component = shape.component_of(slot);
            let mut bodies = BumpVec::with_capacity_in(component.members.len(), self.scratch);
            for member in component.members {
                match shape.births(*member) {
                    Some(body) if body.kind() == ShapeKind::Callable => {
                        bodies.push((body, keeps_scheme(shape, *member)));
                    }
                    _ => return self.leaf_on(scope, BinderAt::Slot(slot)).map(Settled),
                }
            }
            let index = (component.members.iter())
                .position(|member| *member == slot)
                .expect("a binder is a member of its component");
            return match self.knot(scope, &bodies, index as u32, site)? {
                Some(knot) => Ok(Settled(knot)),
                None => self.leaf_on(scope, BinderAt::Slot(slot)).map(Provisional),
            };
        }
        if shape.position_of(slot) == Position::PARAMETER {
            return self.leaf_on(scope, BinderAt::Slot(slot)).map(Settled);
        }
        let tree = match shape.rhs(slot) {
            Some(rhs) => self.tree_in(scope, rhs, Some(BinderAt::Slot(slot)))?,
            None => return self.leaf_on(scope, BinderAt::Slot(slot)).map(Settled),
        };
        // An annotated `LET` binds its value retyped to the annotation.
        let level = self.scope_level(scope);
        match shape.annotation(slot) {
            Some(annotation) => match self.declared(level, annotation) {
                Some(declared) => self
                    .applying()
                    .retyped_tree(tree, declared, site)
                    .map(Settled),
                None => self.leaf_on(scope, BinderAt::Slot(slot)).map(Provisional),
            },
            None => Ok(Settled(tree)),
        }
    }

    /// The tree of the part held in place at `site` of the shape at `level`: its own, built as a
    /// `LET` binder's right-hand side is, with a leaf on its held binder where it falls back.
    pub(super) fn held_tree(&mut self, level: usize, site: Site) -> Read<'o, Tree<'o>> {
        let shape = self.chain[level].shape;
        let node = &shape.body()[self.statement_holding(level, site)];
        let part = part_of(shape, node, site).expect("a part held in place lies in its statement");
        let named = Some(BinderAt::Held(site));
        self.tree_form(
            Scope::Chain(level),
            of_part(shape, part),
            Written::Part(site),
            named,
        )
    }

    /// The tree an expression the load cannot see reads as: a leaf on the `LET` binder `named`
    /// whose whole right-hand side it is, else a leaf no binder names, keyed on `site`.
    fn fallback(
        &mut self,
        scope: Scope<'o, 'graph>,
        named: Option<BinderAt>,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        match named {
            Some(at) => self.leaf_on(scope, at),
            None => Ok(Tree::Leaf(Leaf::Unnamed(site))),
        }
    }

    /// `tree`, or where there is none, the [fallback](Self::fallback).
    fn or_fallback(
        &mut self,
        tree: Option<Tree<'o>>,
        scope: Scope<'o, 'graph>,
        named: Option<BinderAt>,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        match tree {
            Some(tree) => Ok(tree),
            None => self.fallback(scope, named, site),
        }
    }

    /// A leaf on the binder `at` of `scope`'s shape. A `MODULE` or `GROUP` body's binder holds
    /// one value per run of the body, and the body's captures say which run, so its leaf is keyed
    /// on them wherever it is read: on the chain, as the shape enclosing the body resolves them,
    /// and through a body's tree, as the tree holds them.
    pub(super) fn leaf_on(&mut self, scope: Scope<'o, 'graph>, at: BinderAt) -> Read<'o, Tree<'o>> {
        let shape = self.scope_shape(scope);
        let key = match scope {
            _ if shape.kind() != ShapeKind::Module => &[][..],
            // A tree's captures sit by slot, the builder captures first.
            Scope::Body { captures, .. } => {
                let builder = shape.captures().len();
                debug_assert!(captures.is_sorted_by_key(|(at, _)| *at));
                &captures[..captures.partition_point(|(at, _)| (*at as usize) < builder)]
            }
            Scope::Chain(level) => match self.chain[level].captures {
                Some(captures) => captures,
                None => {
                    // A body with no statement reads nothing, so captures nothing.
                    let captures = match shape.body().first() {
                        Some(first) => self.builder_captures(
                            Scope::Chain(level - 1),
                            shape,
                            shape.site_of_node(first),
                        )?,
                        None => &[][..],
                    };
                    self.chain[level].captures = Some(captures);
                    captures
                }
            },
        };
        // A leaf on a module binder the binding leaves unbound is a read of its content, which a
        // binding gives.
        let binder = Binder {
            body: shape.segment(),
            at,
        };
        if let Scope::Chain(level) = scope {
            self.note_leaf(level, binder);
        }
        let within = (!key.is_empty()).then(|| resident(self.lay, Within::Body(key)));
        Ok(Tree::bound(binder, &[], within))
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
            let tree = match self.shapes.capture_source(body.segment(), capture) {
                CaptureSource::Read(coordinate) => self.resolve(scope, coordinate, site)?,
                _ => Tree::Leaf(Leaf::Unnamed(site)),
            };
            captures.push((index as u32, tree));
        }
        #[cfg(test)]
        super::count(|counts| counts.capture_slices += 1);
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
            let Some(instance) = self.born_at(body, keeps_scheme) else {
                return Ok(None);
            };
            let mut captures = BumpVec::with_capacity_in(self.capture_count(body), self.scratch);
            for slot in 0..self.capture_count(body) {
                let capture = CaptureSlot(slot as u32);
                if !self.composes(body, capture) {
                    continue;
                }
                let held = match self.shapes.capture_source(body.segment(), capture) {
                    CaptureSource::Member { index, .. } => NodeCapture::Fellow(index),
                    CaptureSource::Read(coordinate) => {
                        NodeCapture::Tree(self.resolve(scope, coordinate, site)?)
                    }
                    CaptureSource::Hole | CaptureSource::Offered | CaptureSource::Entry { .. } => {
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
            self.types,
            &|segment| self.shapes.shape_at(segment).code_digest(),
            collect(self.lay, members.iter().copied()),
            index,
        )))
    }

    /// The solution the closure `body` is born at: its born instance where the load fixed one,
    /// closed or written over names, and nothing where it is born unquantified or, its binder
    /// keeping its scheme, quantified. `None` where the load cannot say.
    fn born_at(
        &self,
        body: &'graph BodyShape<'graph>,
        keeps_scheme: bool,
    ) -> Option<&'o [Parametric]> {
        match self.born_instance(body) {
            Some(StaticInstance::Closed { solution, .. }) => Some(collect(
                self.lay,
                solution.iter().map(|solved| Parametric::from(*solved)),
            )),
            Some(StaticInstance::Rigid { solution, .. }) => Some(solution),
            None => match self.callable_of(body) {
                Static::Closed(callable) if callable.ktype.as_type().is_some() => Some(&[]),
                Static::Rigid { value, .. } if value.ktype.as_type().is_some() => Some(&[]),
                Static::Closed(_) | Static::Rigid { .. } if keeps_scheme => Some(&[]),
                // A scheme born with no instance is not typed yet, or never typed.
                _ => None,
            },
        }
    }

    /// The tree of the keyworded use `node` of the shape at `level`, the whole right-hand side of
    /// the `LET` binder `named` where one holds it: unfolded where the text selects one
    /// registration there, and the call's own leaf where only an answer selects
    /// ([`call_leaf`](Self::call_leaf)); `None` where it does not unfold. The load's rigid
    /// selection never unfolds it, so the tree is one at load and under every binding.
    fn keyworded(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        named: Option<BinderAt>,
    ) -> Read<'o, Option<Tree<'o>>> {
        let shape = self.chain[level].shape;
        let Some(index) = shape.candidate_lists().position(shape.site_of_node(node)) else {
            return Ok(None);
        };
        let statics = self.use_at(level, index);
        if let Some(undecided) = statics.undecided {
            return self.call_leaf(level, node, undecided, named);
        }
        let Narrowing::Selected(coordinate) = statics.narrowing else {
            return Ok(None);
        };
        let site = shape.site_of_node(node);
        let Some((callee, names, solution)) = self.selected_callee(level, coordinate, site) else {
            return Ok(None);
        };
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
        let unfolded = self.applying().unfold(
            callee,
            &arguments,
            solution.as_deref(),
            shape.site_of_node(node),
        )?;
        let holder = named.map(|at| Binder {
            body: shape.segment(),
            at,
        });
        Ok(unfolded.map(|tree| self.applying().held(tree, holder)))
    }

    /// The tree of the keyworded use `node` of the shape at `level`, whose call an answer selects
    /// among `undecided`: the leaf keyed on its candidates' callees and its arguments' trees, held
    /// by the `LET` binder `named` where one holds it whole, resolved where its types close
    /// ([`resolved_call`](Self::resolved_call)). `None` where a candidate is no callable body or
    /// an argument no evaluated part.
    fn call_leaf(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        selection: &'o Selection<'o>,
        named: Option<BinderAt>,
    ) -> Read<'o, Option<Tree<'o>>> {
        let shape = self.chain[level].shape;
        let site = shape.site_of_node(node);
        let Some(list) = shape.candidates(site) else {
            return Ok(None);
        };
        let wanted = slots(node, self.scratch);
        let mut callees = BumpVec::with_capacity_in(list.candidates.len(), self.scratch);
        for candidate in list.candidates {
            let Candidate::One(coordinate) = candidate else {
                return Ok(None);
            };
            let Some((callee, names, quantifier_map, _)) = self.selected_body(level, *coordinate)
            else {
                callees.push(Callee {
                    body: None,
                    parameters: &[],
                    quantified: false,
                });
                continue;
            };
            let mut parameters = BumpVec::with_capacity_in(wanted.len(), self.scratch);
            for position in 0..wanted.len() {
                let Some((parameter, _)) = names.get(position).and_then(|name| callee.slot(*name))
                else {
                    return Ok(None);
                };
                parameters.push(parameter);
            }
            callees.push(Callee {
                body: Some(callee.segment()),
                parameters: collect(self.lay, parameters.iter().copied()),
                quantified: !quantifier_map.is_empty(),
            });
        }
        let mut arguments = BumpVec::with_capacity_in(wanted.len(), self.scratch);
        for wanted in wanted.iter() {
            let Wanted::Evaluated(part) = wanted else {
                return Ok(None);
            };
            arguments.push(self.tree(level, part)?);
        }
        let called = resident(
            self.lay,
            Called {
                at: shape.segment(),
                callees: collect(self.lay, callees.iter().copied()),
                arguments: collect(self.lay, arguments.iter().copied()),
                selection: *selection,
            },
        );
        let holder = named.map(|at| Binder {
            body: shape.segment(),
            at,
        });
        Ok(Some(self.applying().resolved_call(
            called,
            holder,
            &[],
            site,
        )?))
    }

    /// What the use at `site` of the shape at `level`, which selected `coordinate`, calls: a
    /// registration's callable body, beside its parameter names in slot order and each `FOR ALL`
    /// variable's slot at the closed point the walk solved it to there, or `None` where a point is
    /// not closed. `None` where it calls none ([`selected_body`](Self::selected_body)).
    #[allow(clippy::type_complexity)]
    fn selected_callee(
        &mut self,
        level: usize,
        coordinate: Coordinate,
        site: Site,
    ) -> Option<(
        &'graph BodyShape<'graph>,
        &'o [BinderSymbol],
        Option<BumpVec<'p, (Slot, Parametric)>>,
    )> {
        let (callee, names, quantifier_map, _) = self.selected_body(level, coordinate)?;
        let mut solution = BumpVec::new_in(self.scratch);
        if quantifier_map.is_empty() {
            return Some((callee, names, Some(solution)));
        }
        // A use that never runs its call records no points.
        let Some(points) = self.selection_at(level, site) else {
            return Some((callee, names, None));
        };
        for (name, at) in quantifier_map.iter() {
            let variable = callee.slot(BinderSymbol::Type(name))?.0;
            // A point over names may be closed by one binding and carried under another.
            let Some(point) = points.closed(at) else {
                return Some((callee, names, None));
            };
            solution.push((variable, point.into()));
        }
        Some((callee, names, Some(solution)))
    }

    /// The callable body a use in the shape at `level` that selected `coordinate` calls: a
    /// registration's the chain declares, uncrossed, beside its parameter names in slot order, its
    /// `FOR ALL` names and whether its registered type is closed. `None` where it calls none, or
    /// its registered type is unknown or its parameters unnamed.
    #[allow(clippy::type_complexity)]
    fn selected_body(
        &mut self,
        level: usize,
        coordinate: Coordinate,
    ) -> Option<(
        &'graph BodyShape<'graph>,
        &'o [BinderSymbol],
        ShapeGroupMap<'o>,
        bool,
    )> {
        let followed = self.follow(level, coordinate);
        let Landing::Slot { level: home, slot } = followed.landing else {
            return None;
        };
        let declaring = self.chain[home].shape;
        let (Some(callee), Some(_)) = (declaring.births(slot), declaring.registration(slot)) else {
            return None;
        };
        if followed.crossed || callee.kind() != ShapeKind::Callable {
            return None;
        }
        let (parameters, quantifier_map, closed) = match self
            .resolved_registered(self.registered_cell(declaring, slot))
        {
            Static::Closed(registered) => (registered.parameters, registered.quantifier_map, true),
            Static::Rigid { value, .. } => (value.parameters, value.quantifier_map, false),
            Static::Unknown => return None,
        };
        let ParameterBinding::Named(names) = parameters else {
            return None;
        };
        Some((callee, names, quantifier_map, closed))
    }

    /// What the keyworded use `node` of the shape at `level` passes at the slots of the callable
    /// body its candidate at `coordinate` calls, written over names, where the body's registration
    /// is closed: each `FOR ALL` name's point, by its index in the candidate's group, and each
    /// parameter's argument ([`fixed_argument`](Self::fixed_argument)), sorted by slot. A slot it
    /// passes nothing the trace follows at is left out. `None` where it calls no such body.
    pub(super) fn passing(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        coordinate: Coordinate,
        site: Site,
    ) -> Option<(Segment, BumpVec<'p, (Slot, Passing)>)> {
        // Content is passed only where the registration is closed.
        let Some((callee, names, quantifier_map, true)) = self.selected_body(level, coordinate)
        else {
            return None;
        };
        let scratch = self.scratch;
        let mut passing = BumpVec::new_in(scratch);
        for (name, at) in quantifier_map.iter() {
            if let Some((slot, _)) = callee.slot(BinderSymbol::Type(name)) {
                passing.push((slot, Passing::Point(at as u16)));
            }
        }
        let params = self.parameter_types(callee);
        let wanted = slots(node, scratch);
        for (position, wanted) in wanted.iter().enumerate() {
            let (Wanted::Evaluated(part), Some(name)) = (wanted, names.get(position)) else {
                continue;
            };
            let Some((slot, _)) = callee.slot(*name) else {
                continue;
            };
            // The mapping is the load's alone: it marks nothing.
            let declared = (params.and_then(|params| params.get(name.symbol())))
                .and_then(|declared| judge::static_of(self.types, declared));
            if let Some(passed) = self.fixed_argument(level, part, declared, site) {
                passing.push((slot, passed));
            }
        }
        passing.sort_by_key(|(slot, _)| *slot);
        Some((callee.segment(), passing))
    }

    /// Record what the keyworded use `node` of the shape at `level`, which selected `coordinate`
    /// at `site`, passes at its callee's slots, where it calls a callable body: its
    /// [`passing`](Self::passing) with each `FOR ALL` name's point read
    /// ([`fixed_point`](Self::fixed_point)), and only the content the trace follows.
    pub(super) fn trace(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        coordinate: Coordinate,
        site: Site,
    ) {
        let Some((callee, passing)) = self.passing(level, node, coordinate, site) else {
            return;
        };
        let points = self.selection_at(level, site);
        let mut fixed = BumpVec::with_capacity_in(passing.len(), self.scratch);
        let mut passes = BumpVec::with_capacity_in(passing.len(), self.scratch);
        for (slot, passed) in passing.iter() {
            let point = match *passed {
                Passing::Fixed(passed) => {
                    fixed.push((*slot, passed));
                    passes.push((*slot, Passing::Fixed(passed)));
                    continue;
                }
                Passing::Point(at) => points.and_then(|points| points.points.get(at as usize)),
                Passing::Type(_) | Passing::Carried(_) => {
                    passes.push((*slot, *passed));
                    continue;
                }
            };
            let point = point.and_then(|point| match *point {
                Point::Closed(point) => Some(point.into()),
                Point::Rigid(point) => Some(point),
                Point::Carried(_) => None,
            });
            if let Some(point) = point {
                passes.push((*slot, Passing::Type(point)));
            }
            if let Some(passed) = point.and_then(|point| self.fixed_point(level, point)) {
                fixed.push((*slot, passed));
            }
        }
        let traced = Traced {
            callee,
            call: node.source,
            by_name: false,
            fixed: collect(self.lay, fixed.iter().copied()),
            passes: collect(self.lay, passes.iter().copied()),
        };
        self.record_traced(level, site, traced);
    }

    /// Record what the call by name `node` of the shape at `level` passes at its callee's slots,
    /// where its head's tree is a member of a knot of a callable body of a closed type: each field
    /// of a record literal argument's
    /// ([`fixed_argument`](Self::fixed_argument)), and each `FOR ALL` name at the member's
    /// instance, else at the point `intervals`, the call's solve of its group, gives it. A head
    /// behind a header passes a `FOR ALL` name only at an instance, and no module, since its frame
    /// solves and binds only once retyped. Kept at the head's site, where the text lists the call.
    pub(super) fn trace_by_name(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        intervals: Option<&[Interval]>,
    ) {
        let [head, argument] = node.parts else {
            return;
        };
        // An argument that is no record literal passes nothing the trace follows at any parameter.
        let fields = match &argument.value {
            ExpressionPart::RecordLiteral(fields) => *fields,
            _ => &[],
        };
        let shape = self.chain[level].shape;
        let Some(site) = self.shapes.head_site(shape, &head.value) else {
            return;
        };
        let (headed, tree) = match self.tree(level, &head.value) {
            Ok(Tree::Retyped { source, .. }) => (true, *source),
            Ok(tree) => (false, tree),
            Err(_) => return,
        };
        let Tree::Knot { members, index, .. } = tree else {
            return;
        };
        let member = members[index as usize];
        let callee = self.shape_at(member.body);
        if callee.kind() != ShapeKind::Callable {
            return;
        }
        let Static::Closed(callable) = self.callable_of(callee) else {
            return;
        };
        let scratch = self.scratch;
        let mut fixed = BumpVec::new_in(scratch);
        let mut passing = BumpVec::new_in(scratch);
        if let DeclaredType::Scheme(_) = callable.ktype {
            for (name, index) in callable.quantifier_map.iter() {
                let Some((slot, _)) = callee.slot(BinderSymbol::Type(name)) else {
                    continue;
                };
                let point = match member.instance.get(index) {
                    Some(point) => Some(*point),
                    None if headed => None,
                    None => (intervals.and_then(|intervals| intervals.get(index)))
                        .filter(|interval| {
                            judge::exact(&NoMarks, self.types, self.scratch, **interval)
                        })
                        .map(|interval| interval.upper),
                };
                if let Some(at) = point.and_then(|point| self.fixed_point(level, point)) {
                    fixed.push((slot, at));
                    passing.push((slot, Passing::Fixed(at)));
                }
            }
        }
        let params = self.parameter_types(callee);
        for (name, value) in fields.iter() {
            let Some((slot, position)) = callee.slot(*name) else {
                continue;
            };
            if position != Position::PARAMETER {
                continue;
            }
            // The trace is the load's alone: it marks nothing.
            let declared = (params.and_then(|params| params.get(name.symbol())))
                .and_then(|declared| judge::static_of(self.types, declared));
            // Past a header the frame binds a module once retyped to the type the header shows,
            // so only a type passes.
            let passes = |at: &Fixed| match at {
                Fixed::Content(Content::Type(_)) => true,
                Fixed::Content(Content::Module { .. }) => !headed,
                Fixed::Passed(_) => !headed || matches!(name, BinderSymbol::Type(_)),
            };
            // The trace records content alone: a type written over names passes none.
            let argument = self.fixed_argument(level, value, declared, site);
            if let Some(passed @ (Passing::Type(_) | Passing::Carried(_))) =
                argument.filter(|_| !headed)
            {
                passing.push((slot, passed));
            }
            let passed = match argument {
                Some(Passing::Fixed(at)) => Some(at),
                _ => None,
            };
            if let Some(at) = passed.filter(passes) {
                fixed.push((slot, at));
                passing.push((slot, Passing::Fixed(at)));
            }
        }
        fixed.sort_by_key(|(slot, _)| *slot);
        passing.sort_by_key(|(slot, _)| *slot);
        let traced = Traced {
            callee: callee.segment(),
            call: node.source,
            by_name: true,
            fixed: collect(self.lay, fixed.iter().copied()),
            passes: collect(self.lay, passing.iter().copied()),
        };
        self.record_traced(level, site, traced);
    }

    /// The parameter types of the callable body `callee`'s closed type, which its frame retypes its
    /// arguments to.
    fn parameter_types(
        &self,
        callee: &'graph BodyShape<'graph>,
    ) -> Option<crate::type_lattice::Record<'o, Parametric>> {
        match self.callable_of(callee) {
            Static::Closed(callable) => match callable.ktype.view(self.types).body {
                TypeNode::KFunction { params, .. } => Some(params),
                _ => None,
            },
            _ => None,
        }
    }

    /// What a `FOR ALL` name solved to `point`, in a use in the shape at `level`, passes: its
    /// closed point, or the `FOR ALL` name of the callable owning the shape that `point` is
    /// exactly, passed on. `None` for any other point.
    fn fixed_point(&self, level: usize, point: Parametric) -> Option<Fixed> {
        if let Some(closed) = judge::static_of(self.types, point) {
            return Some(Fixed::Content(Content::Type(closed)));
        }
        let TypeNode::Lexical {
            level: variable, ..
        } = judge::own_variable(self.types, point)
        else {
            return None;
        };
        let owner = self.shape_at(self.shapes.owner(self.chain[level].shape.segment()));
        if owner.kind() != ShapeKind::Callable {
            return None;
        }
        let Some(Target::Local(slot)) = self.shapes.home(owner.segment(), variable) else {
            return None;
        };
        let typed = matches!(owner.slot_name(slot), BinderSymbol::Type(_));
        (typed && owner.position_of(slot) == Position::PARAMETER).then_some(Fixed::Passed(slot))
    }

    /// What the argument `part`, in a use in the shape at `level` at `site`, passes at a parameter
    /// of the closed declared type `declared`: at one declared `Any`, the type it carries
    /// ([`carried_argument`](Self::carried_argument)); else a module whose static type is an exact
    /// signature and whose tree is closed; a type value, closed or written over names; or a
    /// parameter of the callable owning the shape, named bare, at that same closed declared type,
    /// passed on. `None` for anything else, and where `declared` is not closed.
    fn fixed_argument(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        declared: Option<KType>,
        site: Site,
    ) -> Option<Passing> {
        let (types, shape) = (self.types, self.chain[level].shape);
        let declared = declared?;
        if declared == KType::ANY {
            return self.carried_argument(level, part);
        }
        if let Some(slot) = self.shapes.passed_parameter(shape, part) {
            let owner = self.shape_at(self.shapes.owner(shape.segment()));
            let own = (self.parameter_types(owner))
                .and_then(|params| params.get(owner.slot_name(slot).symbol()))
                .and_then(|own| judge::static_of(types, own));
            return (own == Some(declared)).then_some(Passing::Fixed(Fixed::Passed(slot)));
        }
        match self.type_value(level, part) {
            Some(Static::Closed(named)) => {
                return Some(Passing::Fixed(Fixed::Content(Content::Type(named))));
            }
            Some(Static::Rigid { value, .. }) => return Some(Passing::Type(value)),
            Some(Static::Unknown) | None => {}
        }
        let at = (self.part_type(level, shape.site(part)))
            .and_then(|typed| judge::exactly(types, typed))
            .filter(|exact| exact.kind_of(types) == KKind::Signature)?;
        let tree = self.tree(level, part).ok()?;
        // Only a closed tree.
        tree.digest()?;
        // The run's knot member: a retype's bare member, a view or a body itself.
        let mut member = tree;
        while let Tree::Retyped { source, .. } = member {
            member = *source;
        }
        let own = self
            .carried_type(super::views::Carried::Tree(member), site)
            .ok()??;
        debug_assert!(
            !matches!(tree, Tree::Body { .. }) || at == own,
            "a bare module is typed its own type: a differing annotation retypes it"
        );
        Some(Passing::Fixed(Fixed::Content(Content::Module {
            content: member.digest()?,
            at,
            own,
            declared,
        })))
    }

    /// What the argument `part`, read in the shape at `level`, passes at a parameter declared
    /// `Any`, the name of the type its value carries: its static type, closed or written over
    /// names, which is that type where a binding makes it exact. `None` for a type value or a
    /// module, whose frame binds no such type: a module's static type is the signature it shows,
    /// not the type it carries.
    fn carried_argument(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Option<Passing> {
        let (types, shape) = (self.types, self.chain[level].shape);
        if self.type_value(level, part).is_some() {
            return None;
        }
        let typed = self.part_type(level, shape.site(part))?;
        let module = judge::static_of(types, typed.upper)
            .is_some_and(|closed| closed.kind_of(types) == KKind::Signature);
        let type_valued = matches!(judge::own_variable(types, typed.upper), TypeNode::OfKind(_));
        (!module && !type_valued).then_some(Passing::Carried(typed))
    }

    /// The type the type value `part`, read in the shape at `level`, names, closed or written over
    /// names: `None` for any other part.
    fn type_value(
        &self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Option<StaticType<'o>> {
        let shape = self.chain[level].shape;
        match part {
            ExpressionPart::Type(_) => self.head_typed(level, part),
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                Some(self.resolved_type(self.type_cell(shape, shape.site(part))))
            }
            _ => None,
        }
    }

    /// The tree of the call by name `node` of the shape at `level`, unfolded where its head's tree
    /// is a closure the chain declares, typed closed or over names, and its argument a record
    /// literal: `None` where it does not unfold.
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
        // A scheme's call by name solves its points at the run: none is written here.
        let solution = match self.callable_type(callee) {
            Some(DeclaredType::Type(_)) => Some(&[][..]),
            Some(DeclaredType::Scheme(_)) => None,
            None => return Ok(None),
        };
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
        let site = self.node_site(level, node);
        self.applying().unfold(callee, &arguments, solution, site)
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

    /// `callee`'s function type as the load fixed it ([`callable_type`](graph::callable_type)).
    fn callable_type(&self, callee: &'graph BodyShape<'graph>) -> Option<DeclaredType<Parametric>> {
        graph::callable_type(self.callable_of(callee))
    }
}

impl<'p, 'o: 'p, 'x, 'graph: 'o + 'p, 'tier: 'o, 'pair, 'reg, S: Store<'o>>
    Pass<'p, 'o, 'x, 'graph, 'tier, 'pair, 'reg, S>
{
    /// An application of trees over the walk's facts: the records it is writing.
    pub(super) fn applying(&mut self) -> Applying<'_, 'p, 'o, 'x, 'graph, 'tier, 'pair, 'reg, S> {
        Apply {
            types: self.types,
            scratch: self.scratch,
            lay: self.lay,
            shapes: self.shapes,
            facts: self,
        }
    }
}

/// [`Pass::applying`]'s application.
pub(super) type Applying<'f, 'p, 'o, 'x, 'graph, 'tier, 'pair, 'reg, S> = Apply<
    'f,
    'p,
    'x,
    'graph,
    'tier,
    'pair,
    'reg,
    <S as Store<'o>>::Lay,
    Pass<'p, 'o, 'x, 'graph, 'tier, 'pair, 'reg, S>,
>;

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> TreeFacts<'p, 'o>
    for Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// The member's tree read off the body's text, its captures the tree's.
    fn member(
        &mut self,
        body: Segment,
        captures: &'o [(u32, Tree<'o>)],
        slot: Slot,
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let shape = self.shape_at(body);
        self.slot_tree(Scope::Body { shape, captures }, slot, site)
    }

    fn callable(&self, body: Segment) -> StaticCallable<'o> {
        self.callable_of(self.shape_at(body))
    }

    fn signature(&self, body: Segment) -> Option<KType> {
        self.signature_of(self.shape_at(body))
    }

    /// The summary of `callee`, a kept call's callee, walking it first where the walk has not:
    /// `None` where the call does not unfold — the callee is still being typed, or recursive — or
    /// where the walk is typing unfilled code.
    fn summary(&mut self, callee: Segment) -> Read<'o, Option<Tree<'o>>> {
        let callee = self.shape_at(callee);
        if self.unfilled || !self.kept_call(callee)? {
            return Ok(None);
        }
        Ok(self.summary_of(callee).flatten())
    }

    fn held(&self, _: Segment, _: Identity) -> Option<Tree<'o>> {
        None
    }

    fn bound(&self, _: crate::scope::Unknown) -> Option<crate::scope::Bound<'o>> {
        None
    }

    /// The walk binds nothing, so a type stands as it is.
    fn rebind(&self, _: Segment, _: DeclaredType<Parametric>) -> crate::scope::Closed {
        crate::scope::Closed::Same
    }

    /// The fit the walk memoized, its gathering's tag gathered.
    fn fitted(&mut self, key: (KType, Identity)) -> Option<Fit> {
        let (from, held, revealed, tag) = self.view_trees.get(&key).copied()?;
        self.gather(tag);
        Some((from, held, revealed))
    }

    fn file_fit(&mut self, key: (KType, Identity), (from, held, revealed): Fit) {
        let tag = self.innermost();
        self.view_trees.insert(key, (from, held, revealed, tag));
    }

    /// What the walk memoized the view to bind, its gathering's tag gathered.
    fn viewed(&mut self, identity: Identity) -> Option<Option<Exposure<'o, 'p>>> {
        let (viewed, tag) = self.views.get(&identity).copied()?;
        self.gather(tag);
        Some(viewed)
    }

    fn file_viewed(&mut self, identity: Identity, viewed: Option<Exposure<'o, 'p>>) {
        let tag = self.innermost();
        self.views.entry(identity).or_insert((viewed, tag));
    }

    fn note(&self, note: Note) {
        #[cfg(test)]
        super::count(|counts| match note {
            Note::Fit => {
                counts.source_reads += 1;
                counts.fits += 1;
            }
            Note::ViewRead => counts.view_reads += 1,
            Note::ChainLabels(len) => counts.chain_labels += len,
        });
        #[cfg(not(test))]
        let _ = note;
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
