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
//! A keyworded call only an answer selects among is a **call leaf**, keyed on its candidates'
//! callees and its argument trees, so two writes of one call are one tree at load and under every
//! binding; a witness's selection never unfolds it. A binding substituting the leaf answers the
//! choice closed ([`runs`]) and unfolds the candidate it selects, as a fresh load does; where none
//! is selected, the run binds the `LET` holding the call at its statement.
//!
//! At each call the load's walk resolves to one callable body — a keyworded use it selected, or
//! a call by name whose head's tree is a closure — it records what the call passes at each of the
//! callee's slots ([`Traced`]): load content, or a `FOR ALL` name or parameter of the callable
//! owning the call passed on bare, a parameter only at its callee's own closed declared type.
//! The [trace](../../build/README.md#the-trace) folds those records.
//!
//! Trees on the chain are memoized per shape and site, and each binder's tree once on the chain and
//! once through each body's tree, except where a cycle cut on an outer binder, a fallback the
//! walk's progress could still sharpen, or a leaf keyed on the read site shaped it. A binder
//! already being read reads as a leaf, so no tree is built through itself.

use crate::elaborate::judge::{self, NoMarks};
use crate::elaborate::signature_member;
use crate::memory::{BumpBackedMap, BumpVec, bump_table, collect, resident};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    Bare, Binder, BodyShape, Called, Callee, Candidate, CaptureSlot, CaptureSource, Content,
    Coordinate, Fixed, Identity, KnotNode, Leaf, Narrowing, NodeCapture, ParameterBinding, Point,
    Position, Revealed, Run, Running, Segment, ShapeError, ShapeGroupMap, ShapeKind, Shapes, Site,
    Slot, Static, StaticCallable, StaticInstance, Store, Target, Traced, Tree, UndecidedCall,
    Unfolded, Within, runs,
};
use crate::symbols::{BinderSymbol, ValueSymbol};
use crate::type_lattice::{DeclaredType, Interval, KKind, KType, Parametric, TypeNode};
use crate::values::digest::{literal_digest, type_digest};
use crate::values::{retype_target, retypes_by_header};

use super::super::evaluate::{Form, Wanted, of_node, of_part, slots};
use super::{Landing, Pass, address};
use crate::knot::module::view::{self, Ascription, Unascribable};

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
        named: Option<Slot>,
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
        named: Option<Slot>,
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
        named: Option<Slot>,
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
                let read = self.read_member(through, label, site)?;
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
                            Ascription::Opaque => self.view_tree(application, source, site)?,
                            // `:!` is a retype.
                            Ascription::Transparent => {
                                self.retyped_tree(source, application, site)?
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
        // A standing binder's content tree is read off the record of the unit binding it.
        if let Scope::Chain(level) = scope
            && let Some((tree, tag)) = self.stood_content(level, slot)
        {
            self.gather(tag);
            return Ok(tree);
        }
        if let Some(at) = self.reading.iter().position(|reading| *reading == key) {
            self.shaped.cut = self.shaped.cut.min(at);
            return self.leaf_on(scope, slot);
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
        // A binder the binding binds holds what it binds: a type name its type, a module binder
        // the closed content tree it binds.
        let unknown = crate::scope::Unknown {
            segment: shape.segment(),
            at: crate::scope::Address::Slot(slot),
        };
        match self.bound_to(unknown) {
            Some(crate::scope::Bound::Module { tree, .. }) => return Ok(Settled(*tree)),
            Some(crate::scope::Bound::Type(bound)) => {
                return Ok(Settled(named_type(self.types, bound)));
            }
            None => {}
        }
        if let BinderSymbol::Type(name) = shape.slot_name(slot) {
            return match self.declared_cell(shape, slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Ok(Settled(named_type(self.types, handle)))
                }
                Static::Closed(_) => self.leaf_on(scope, slot).map(Settled),
                // A type name written over names is the type value it names, which a binding
                // closes as a fresh load reads it.
                Static::Rigid { value, .. } => Ok(Settled(type_tree(self.types, value))),
                Static::Unknown => match self.name_variable(shape, slot, name) {
                    Some((variable, true)) => Ok(Settled(type_tree(self.types, variable))),
                    Some((variable, false)) => Ok(Provisional(type_tree(self.types, variable))),
                    None => self.leaf_on(scope, slot).map(Provisional),
                },
            };
        }
        if let Some(body) = shape.births(slot) {
            if body.kind() == ShapeKind::Module {
                // A body not yet typed has not laid its type captures down.
                if !self.done(body) {
                    return self.leaf_on(scope, slot).map(Provisional);
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
                    _ => return self.leaf_on(scope, slot).map(Settled),
                }
            }
            let index = (component.members.iter())
                .position(|member| *member == slot)
                .expect("a binder is a member of its component");
            return match self.knot(scope, &bodies, index as u32, site)? {
                Some(knot) => Ok(Settled(knot)),
                None => self.leaf_on(scope, slot).map(Provisional),
            };
        }
        if shape.position_of(slot) == Position::PARAMETER {
            return self.leaf_on(scope, slot).map(Settled);
        }
        let tree = match shape.rhs(slot) {
            Some(rhs) => self.tree_in(scope, rhs, Some(slot))?,
            None => return self.leaf_on(scope, slot).map(Settled),
        };
        // An annotated `LET` binds its value retyped to the annotation.
        let level = self.scope_level(scope);
        match shape.annotation(slot) {
            Some(annotation) => match self.declared(level, annotation) {
                Some(declared) => self.retyped_tree(tree, declared, site).map(Settled),
                None => self.leaf_on(scope, slot).map(Provisional),
            },
            None => Ok(Settled(tree)),
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
        if let Scope::Chain(level) = scope {
            self.note_leaf(level, slot);
        }
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
        self.read_step(tree, label, &[], &mut None, site)
    }

    /// `tree` read through each of `labels` in turn, at the part at `site`: `None` where a step
    /// reads nothing. A leaf's chain is extended as a prefix of `labels`, or of one chain joining
    /// the leaf's to them, so the read lays O(n) labels for an n-step chain.
    pub(super) fn read_chain(
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
                let declared =
                    signature_member(self, types, scratch, carried, BinderSymbol::Value(label))
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
        #[cfg(test)]
        super::count(|counts| counts.chain_labels += len);
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
    pub(super) fn retyped_tree(
        &mut self,
        tree: Tree<'o>,
        declared: crate::type_lattice::Parametric,
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
                let own = match self.callable_of(self.shape_at(node.body)) {
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
    pub(super) fn view_tree(
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
    pub(super) fn view_tree_held(
        &mut self,
        application: Parametric,
        source: Tree<'o>,
        site: Site,
    ) -> Read<'o, (Tree<'o>, Option<KType>, Option<Revealed>)> {
        // An application written over names has no fit until a binding closes it: open, a view a
        // binding substitutes, so reading it closed judges nothing.
        let Some(application) = judge::static_of(self.types, application) else {
            let underlying = resident(self.lay, self.underlying_tree(source));
            let source = resident(self.lay, source);
            return Ok((
                Tree::view(self.types, application, source, underlying, None),
                None,
                None,
            ));
        };
        let key = (application, source.identity());
        let (from, held, revealed, seed) = match self.view_trees.get(&key).copied() {
            Some((from, held, revealed, tag)) => {
                self.gather(tag);
                (from, held, revealed, None)
            }
            None => {
                #[cfg(test)]
                super::count(|counts| {
                    counts.source_reads += 1;
                    counts.fits += 1;
                });
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
                let tag = self.innermost();
                // A leaf no binder names is keyed on its site, which the identity does not tell apart.
                if key.1 != Identity::Unnamed {
                    self.view_trees
                        .insert(key, (from, operand.held, revealed, tag));
                }
                (
                    from,
                    operand.held,
                    revealed,
                    exposure.map(|exposure| (exposure, tag)),
                )
            }
        };
        // Built over this source itself, which another of its identity may not be.
        let underlying = self.underlying_tree(source);
        let tree = Tree::view(
            self.types,
            application.into(),
            resident(self.lay, source),
            resident(self.lay, underlying),
            from,
        );
        // A closed view's underlying is closed, so its fit read what the view binds too.
        if let (identity @ Identity::Closed(_), Some((viewed, tag))) = (tree.identity(), seed) {
            self.views.entry(identity).or_insert((Some(viewed), tag));
        }
        Ok((tree, held, revealed))
    }

    /// The tree of the module a view over `source` is built over: an opaque view's underlying, a
    /// module a frame binds that is one by its underlying's digest, and any other module itself,
    /// behind any header.
    pub(super) fn underlying_tree(&self, source: Tree<'o>) -> Tree<'o> {
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
                    self.lay,
                    Bare {
                        signature: Some(sealing.underlying_signature),
                        sealing: None,
                    },
                )),
            ),
            bare => bare,
        }
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
    /// ([`call_leaf`](Self::call_leaf)); `None` where it does not unfold. A witness's selection
    /// never unfolds it, so the tree is one at load and under every binding.
    fn keyworded(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        named: Option<Slot>,
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
        self.unfold(
            callee,
            &arguments,
            solution.as_deref(),
            shape.site_of_node(node),
        )
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
        undecided: &'o UndecidedCall<'o>,
        named: Option<Slot>,
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
                undecided: *undecided,
            },
        );
        let holder = named.map(|slot| Binder {
            body: shape.segment(),
            slot,
        });
        Ok(Some(self.resolved_call(called, holder, &[], site)?))
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

    /// Record what the keyworded use `node` of the shape at `level`, which selected `coordinate`
    /// at `site`, passes at its callee's slots, where it calls a callable body: each `FOR ALL`
    /// name's point ([`fixed_point`](Self::fixed_point)) and each parameter's argument
    /// ([`fixed_argument`](Self::fixed_argument)). A slot it passes nothing the trace follows at
    /// is left out.
    pub(super) fn trace(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        coordinate: Coordinate,
        site: Site,
    ) {
        // The trace records content, which only a closed registration passes.
        let Some((callee, names, quantifier_map, true)) = self.selected_body(level, coordinate)
        else {
            return;
        };
        let scratch = self.scratch;
        let mut fixed = BumpVec::new_in(scratch);
        if !quantifier_map.is_empty()
            && let Some(points) = self.selection_at(level, site)
        {
            for (name, at) in quantifier_map.iter() {
                let (Some((slot, _)), Some(point)) =
                    (callee.slot(BinderSymbol::Type(name)), points.points.get(at))
                else {
                    continue;
                };
                let point = match *point {
                    Point::Closed(point) => Some(point.into()),
                    Point::Rigid(point) => Some(point),
                    Point::Carried(_) => None,
                };
                if let Some(at) = point.and_then(|point| self.fixed_point(level, point)) {
                    fixed.push((slot, at));
                }
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
            // The trace is the load's alone, which no resumption re-runs: it marks nothing.
            let declared = (params.and_then(|params| params.get(name.symbol())))
                .and_then(|declared| judge::static_of(self.types, declared));
            if let Some(at) = self.fixed_argument(level, part, declared, site) {
                fixed.push((slot, at));
            }
        }
        fixed.sort_by_key(|(slot, _)| *slot);
        let traced = Traced {
            callee: callee.segment(),
            call: node.source,
            by_name: false,
            fixed: collect(self.lay, fixed.iter().copied()),
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
        if let DeclaredType::Scheme(_) = callable.ktype {
            for (name, index) in callable.quantifier_map.iter() {
                let Some((slot, _)) = callee.slot(BinderSymbol::Type(name)) else {
                    continue;
                };
                let point = match member.instance.get(index) {
                    Some(point) => Some(*point),
                    None if headed => None,
                    None => (intervals.and_then(|intervals| intervals.get(index)))
                        .filter(|interval| judge::exact(&NoMarks, self.types, **interval))
                        .map(|interval| interval.upper),
                };
                if let Some(at) = point.and_then(|point| self.fixed_point(level, point)) {
                    fixed.push((slot, at));
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
            // The trace is the load's alone, which no resumption re-runs: it marks nothing.
            let declared = (params.and_then(|params| params.get(name.symbol())))
                .and_then(|declared| judge::static_of(self.types, declared));
            // Past a header the frame binds a module once retyped to the type the header shows,
            // so only a type passes.
            let passes = |at: &Fixed| match at {
                Fixed::Content(Content::Type(_)) => true,
                Fixed::Content(Content::Module { .. }) => !headed,
                Fixed::Passed(_) => !headed || matches!(name, BinderSymbol::Type(_)),
            };
            if let Some(at) = self
                .fixed_argument(level, value, declared, site)
                .filter(passes)
            {
                fixed.push((slot, at));
            }
        }
        fixed.sort_by_key(|(slot, _)| *slot);
        let traced = Traced {
            callee: callee.segment(),
            call: node.source,
            by_name: true,
            fixed: collect(self.lay, fixed.iter().copied()),
        };
        self.record_traced(level, site, traced);
    }

    /// `callee`'s function type as the load fixed it, closed or written over the run's variables:
    /// `None` where the load fixed none.
    fn callable_type(&self, callee: &'graph BodyShape<'graph>) -> Option<DeclaredType<Parametric>> {
        match self.callable_of(callee) {
            Static::Closed(callable) => Some(callable.ktype.map(Parametric::from)),
            Static::Rigid { value, .. } => Some(value.ktype),
            Static::Unknown => None,
        }
    }

    /// `ty`, a position of `callee`'s type, at the call solving its `FOR ALL` variables to
    /// `solution`: each variable written in, or `None` where one has no point there, which a
    /// position read off its scheme would name outside the scheme.
    fn at_solution(
        &self,
        callee: &'graph BodyShape<'graph>,
        ty: Parametric,
        solution: &[(Slot, Parametric)],
    ) -> Option<Parametric> {
        let quantified = match self.callable_of(callee) {
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
    /// of the closed declared type `declared`: a module whose static type is an exact signature and
    /// whose tree is closed; a closed type value; or a parameter of the callable owning the shape,
    /// named bare, at that same closed declared type, passed on. `None` for anything else, and
    /// where `declared` is not closed.
    fn fixed_argument(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        declared: Option<KType>,
        site: Site,
    ) -> Option<Fixed> {
        let (types, shape) = (self.types, self.chain[level].shape);
        let declared = declared?;
        if let Some(slot) = self.shapes.passed_parameter(shape, part) {
            let owner = self.shape_at(self.shapes.owner(shape.segment()));
            let own = (self.parameter_types(owner))
                .and_then(|params| params.get(owner.slot_name(slot).symbol()))
                .and_then(|own| judge::static_of(types, own));
            return (own == Some(declared)).then_some(Fixed::Passed(slot));
        }
        if let Some(named) = self.type_value(level, part) {
            return Some(Fixed::Content(Content::Type(named)));
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
        Some(Fixed::Content(Content::Module {
            content: member.digest()?,
            at,
            own,
            declared,
        }))
    }

    /// The closed type the type value `part`, read in the shape at `level`, names: `None` for any
    /// other part.
    fn type_value(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<KType> {
        let shape = self.chain[level].shape;
        match part {
            ExpressionPart::Type(_) => self.head_handle(level, part),
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match self.resolved_type(self.type_cell(shape, shape.site(part))) {
                    Static::Closed(handle) => Some(handle),
                    _ => None,
                }
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
        self.unfold(callee, &arguments, solution, self.node_site(level, node))
    }

    /// `callee`'s summary with `arguments` and `solution` substituted, for a call at `site`:
    /// `None` where the callee has no summary. A solution the load cannot close, `None`, unfolds
    /// only a summary that reads none of it: one over its arguments alone, whatever the points.
    fn unfold(
        &mut self,
        callee: &'graph BodyShape<'graph>,
        arguments: &[(Slot, Tree<'o>)],
        solution: Option<&[(Slot, Parametric)]>,
        site: Site,
    ) -> Read<'o, Option<Tree<'o>>> {
        let Some(summary) = self.summary(callee)? else {
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

    /// Whether `tree`, `callee`'s summary, reads what its call solves: a type written over names, a
    /// type value, which may name `callee`'s own `FOR ALL` names, or a leaf on a binder of `callee`
    /// or a body in it other than a parameter, which a call keys on its solution or reads it from.
    fn reads_solution(&self, tree: &Tree<'o>, callee: &'graph BodyShape<'graph>) -> bool {
        if tree.closed() {
            return false;
        }
        if self.writes_types(tree) {
            return true;
        }
        let own = match tree {
            Tree::TypeValue { .. } => true,
            Tree::Leaf(Leaf::Bound { binder, .. }) => {
                let parameter = binder.body == callee.segment() && {
                    let named = callee.slot_name(binder.slot);
                    matches!(named, BinderSymbol::Value(_))
                        && callee.slot(named).map(|(_, at)| at) == Some(Position::PARAMETER)
                };
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

    /// `tree` with `by` substituted, each view and retype over a substituted leaf built again as
    /// a fresh load builds it. A call: `tree` is a summary of its callee, each parameter's leaf
    /// read by its argument's tree through the leaf's chain, each `FOR ALL` variable's by its
    /// point, and every other leaf of the callee's body keyed on the call. The walk's binding: each
    /// leaf on a binder it binds read by what it binds. `done` holds each subtree substituted
    /// already, by identity, so a shared subtree is substituted once.
    pub(super) fn substituted(
        &mut self,
        tree: Tree<'o>,
        call: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'p, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        let identity = tree.identity();
        // A closed tree holds no leaf to substitute.
        if let Identity::Closed(_) = identity {
            return Ok(tree);
        }
        if let Some(substituted) = done.get(&identity) {
            return Ok(*substituted);
        }
        let substituted = match tree {
            Tree::Digest(..) | Tree::Leaf(Leaf::Unnamed(_)) => tree,
            Tree::TypeValue { named, .. } => {
                type_tree(self.types, self.substituted_type(named, call))
            }
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
                    let instance =
                        (node.instance.iter()).map(|solved| self.substituted_type(*solved, call));
                    nodes.push(KnotNode {
                        instance: collect(self.lay, instance),
                        captures: collect(self.lay, captures.iter().copied()),
                        ..*node
                    });
                }
                Tree::knot(
                    self.types,
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
                let application = self.substituted_type(application, call);
                self.view_tree(application, source, site)?
            }
            Tree::Retyped {
                carried, source, ..
            } => {
                let source = self.substituted(*source, call, site, done)?;
                let carried = self.substituted_type(carried, call);
                self.retyped_tree(source, carried, site)?
            }
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within,
                ..
            }) => self.substituted_leaf(binder, chain, within, call, site, done)?,
            Tree::Leaf(Leaf::Call {
                call: called,
                holder,
                chain,
                ..
            }) => self.substituted_call(called, holder, chain, call, site, done)?,
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
        done: &mut BumpBackedMap<'p, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        let call = match by {
            Substitution::Call(call) => call,
            Substitution::Binding(segment) => {
                return self.rebound_leaf(binder, chain, within, segment, site, done);
            }
        };
        if binder.body == call.callee {
            let callee = self.shape_at(call.callee);
            if let Some((_, argument)) =
                (call.arguments.iter()).find(|(slot, _)| *slot == binder.slot)
            {
                // The frame binds the argument retyped to the parameter's declared type.
                let declared = match self
                    .callable_type(callee)
                    .map(|ty| ty.view(self.types).body)
                {
                    Some(TypeNode::KFunction { params, .. }) => params
                        .get(callee.slot_name(binder.slot).symbol())
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
            None if self.shape_at(binder.body).kind() == ShapeKind::Module => None,
            None if inside(self.shapes, binder.body, call.callee) => Some(Within::Call(*call)),
            None => None,
        };
        let within = within.map(|within| resident(self.lay, within));
        Ok(Tree::bound(binder, chain, within))
    }

    /// [`substituted_leaf`](Self::substituted_leaf) under the walk's binding: a leaf on a binder it
    /// binds, read in the run being typed, is what the binder holds there, as
    /// [`binder_tree`](Self::binder_tree) reads it, read through the leaf's chain.
    fn rebound_leaf(
        &mut self,
        binder: Binder,
        chain: &'o [ValueSymbol],
        within: Option<&'o Within<'o>>,
        segment: Segment,
        site: Site,
        done: &mut BumpBackedMap<'p, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        let Some(within) = within else {
            let unknown = crate::scope::Unknown {
                segment: binder.body,
                at: crate::scope::Address::Slot(binder.slot),
            };
            let bound = match self.bound_to(unknown) {
                Some(crate::scope::Bound::Module { tree, .. }) => Some(*tree),
                Some(crate::scope::Bound::Type(bound)) => Some(named_type(self.types, bound)),
                None => None,
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

    /// `ty`, a type a tree holds, with `by` written in: the walk's binding read from the record's
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
        match self.rebind(segment, ty) {
            crate::scope::Closed::Type(closed) => closed.map(Parametric::from),
            crate::scope::Closed::Partial(partial) => partial,
            crate::scope::Closed::Same => ty,
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
        let callee = self.shape_at(call.callee);
        let solved = |level: usize, name: crate::symbols::TypeSymbol| {
            (call.solution.iter())
                .find(|(slot, _)| {
                    callee.slot_name(*slot) == BinderSymbol::Type(name)
                        && (self.shapes).level(call.callee, crate::scope::Address::Slot(*slot))
                            == Some(level)
                })
                .map(|(_, point)| Interval::point(*point))
        };
        let read = judge::read_points(
            self.types,
            self.scratch,
            typed,
            crate::type_lattice::Side::Above,
            &mut |variable| match variable {
                crate::type_lattice::Variable::Lexical { level, name, .. } => solved(level, name),
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
        done: &mut BumpBackedMap<'p, Identity, Tree<'o>>,
    ) -> Read<'o, Tree<'o>> {
        if let (Substitution::Binding(_), Some(holder)) = (by, holder) {
            let unknown = crate::scope::Unknown {
                segment: holder.body,
                at: crate::scope::Address::Slot(holder.slot),
            };
            if let Some(crate::scope::Bound::Module { tree, .. }) = self.bound_to(unknown)
                && let Some(read) = self.read_chain(*tree, chain, site)?
            {
                return Ok(read);
            }
        }
        let mut arguments = BumpVec::with_capacity_in(called.arguments.len(), self.scratch);
        for tree in called.arguments {
            arguments.push(self.substituted(*tree, by, site, done)?);
        }
        let mut kept = BumpVec::with_capacity_in(called.undecided.kept.len(), self.scratch);
        for running in called.undecided.kept {
            let mut read = BumpVec::with_capacity_in(running.arguments.len(), self.scratch);
            read.extend(running.arguments.iter().map(|argument| Interval {
                lower: self.substituted_type(argument.lower, by),
                upper: self.substituted_type(argument.upper, by),
            }));
            kept.push(Running {
                shape: self.substituted_declared(running.shape, by),
                arguments: collect(self.lay, read.iter().copied()),
                ..*running
            });
        }
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
                undecided: UndecidedCall {
                    kept: collect(self.lay, kept.iter().copied()),
                    ..called.undecided
                },
                ..*called
            },
        );
        self.resolved_call(called, holder, chain, site)
    }

    /// The tree of the result of `called`, held whole by `holder`, read through `chain`: the
    /// candidate the choice over its types closed selects ([`runs`]) unfolded, as a fresh load
    /// unfolds the call it selects; else the leaf keyed on the call.
    pub(super) fn resolved_call(
        &mut self,
        called: &'o Called<'o>,
        holder: Option<Binder>,
        chain: &'o [ValueSymbol],
        site: Site,
    ) -> Read<'o, Tree<'o>> {
        let leaf = Tree::called(called, holder, chain);
        let undecided = called.undecided;
        let Some(answers) = runs(self.types, self.scratch, undecided.kept, undecided.unfilled)
        else {
            return Ok(leaf);
        };
        let Some(at) = answers.iter().position(|run| *run == Run::Selected) else {
            return Ok(leaf);
        };
        let callee = called.callees[usize::from(undecided.positions[at])];
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
        let Some(unfolded) = self.unfold(self.shape_at(body), &arguments, solution, site)? else {
            return Ok(leaf);
        };
        Ok(self.read_chain(unfolded, chain, site)?.unwrap_or(leaf))
    }

    /// `within`, which run of a leaf's body holds its binder, with `by` substituted in the trees
    /// that key it.
    fn substituted_within(
        &mut self,
        within: Within<'o>,
        by: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'p, Identity, Tree<'o>>,
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
        call: Substitution<'_, 'o>,
        site: Site,
        done: &mut BumpBackedMap<'p, Identity, Tree<'o>>,
    ) -> Read<'o, &'o [(u32, Tree<'o>)]> {
        let mut substituted = BumpVec::with_capacity_in(captures.len(), self.scratch);
        for (slot, tree) in captures {
            substituted.push((*slot, self.substituted(*tree, call, site, done)?));
        }
        Ok(collect(self.lay, substituted.iter().copied()))
    }
}

/// What [`substituted`](Pass::substituted) writes into a tree's leaves: a call the load unfolds,
/// or the walk's binding, read from the segment of the record holding the tree, which the binding
/// also writes into each type the tree holds.
#[derive(Clone, Copy)]
pub(super) enum Substitution<'c, 'o> {
    Call(&'c Unfolded<'o>),
    Binding(Segment),
}

/// The tree of a type value naming `named`: its digest where it is closed, else the type value
/// written over names, which a binding closes to that digest.
fn type_tree<'o>(types: &crate::values::Chain<'_, '_, '_, '_>, named: Parametric) -> Tree<'o> {
    match judge::static_of(types, named) {
        Some(closed) => named_type(types, closed),
        None => Tree::type_value(named),
    }
}

/// The tree of a type value naming `named` at its own kind, as the run births it.
fn named_type<'o>(types: &crate::values::Chain<'_, '_, '_, '_>, named: KType) -> Tree<'o> {
    Tree::Digest(
        type_digest(named, KType::of_kind(named.kind_of(types))),
        None,
    )
}

/// Whether `body` is `callee` or a body nested in it, at any depth: a binder bound once per call
/// of `callee`. Walks up from `body`.
fn inside(shapes: &Shapes<'_>, mut body: Segment, callee: Segment) -> bool {
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
