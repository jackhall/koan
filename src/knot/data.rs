//! A knot's data members: a `LET` whose right-hand side, through one-part groups, is a list, dict or
//! record literal or a nominal construction `(Head payload)`.
//!
//! **Staging** reads a member's right-hand side into scratch with no writer in reach. A literal waits
//! to be lowered; a mention of a fellow member is an edge; any other mention is the word the
//! activation reads, or a refusal while its binder runs; a `FN` that captures a
//! fellow member is a function node of the knot; a part the walk cannot build itself — a call, a
//! keyword form, any other `FN` — is asked of the caller's evaluator by site. Every constructor on
//! the path from a member's root to a fellow mention or such a `FN` is an anonymous node of the same
//! knot; that node and the function node are indexed after the members in the order the walk meets
//! them, and the cell that held each holds an edge. A nested constructor with no edge below it
//! stays an ordinary value.
//!
//! **Memos** follow the nominal cut. A function's memo is its signature type and a tagged node's is
//! the head it names, both known before the knot exists. A *derived* node's memo is read
//! off its cells, an edge contributing its target's memo: a container's is what the plain door of
//! its kind would memoize, and a construction through a family's is the application the
//! construction rule solves from its payload. So derived memos are computed in reverse topological
//! order over the edges between derived nodes, and a cycle of derived nodes alone has no finite
//! type and refuses the tie.
//!
//! **Constructions** are checked by [`values::construction`](crate::values::construction), the one
//! rule an ordinary construction goes through too, before anything is written.

use crate::memory::{BumpAllocator, BumpVec, KnotPlan, Writer, strongly_connected_components};
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::scope::{BodyShape, CaptureSource, Component, Coordinate, ShapeKind, Site, Target};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{
    Circular, ConstructionRefused, Dict, Key, Link, List, Record, Tagged, TypeValue, Value,
    construction, dict_type, kept_entries, list_type, part_ktype, record_type, solves_identity,
};

use super::{Eager, KActivationView, KValue, Knotted, Supplied, Untieable};

/// One part of a data member's right-hand side, read and not yet written.
pub(super) enum Staged<'graph, 'cell, 'x> {
    /// A scalar or string literal, or a quote, lowered where it is written.
    Literal(&'graph ExpressionPart<'graph>),
    Value(KValue<'graph, 'cell>),
    /// The knot node at this index.
    Edge(u32),
    List(BumpVec<'x, Staged<'graph, 'cell, 'x>>),
    /// Entries in written order, each key kept only at its last occurrence.
    Dict(BumpVec<'x, (Key<'cell>, Staged<'graph, 'cell, 'x>)>),
    Record(BumpVec<'x, (BinderSymbol, Staged<'graph, 'cell, 'x>)>),
    /// A nominal construction at `site`.
    Tagged {
        head: &'cell TypeValue,
        site: Site,
        payload: &'x Staged<'graph, 'cell, 'x>,
    },
    /// A `FN` that captures a fellow member: a function node of this knot, running `body`. Only
    /// ever a node's shape, reached through an edge.
    Function(&'graph BodyShape<'graph>),
}

/// A staged data node: the member whose right-hand side holds it, and its constructor.
pub(super) struct Node<'graph, 'cell, 'x> {
    pub owner: u32,
    pub shape: Staged<'graph, 'cell, 'x>,
}

/// Every knot node by index: `None` for a function member, otherwise a data node or a function node
/// a data member holds.
pub(super) type Nodes<'graph, 'cell, 'x> = BumpVec<'x, Option<Node<'graph, 'cell, 'x>>>;

impl<'graph> Node<'graph, '_, '_> {
    /// The callable body this node runs, if it is a function node.
    pub(super) fn function(&self) -> Option<&'graph BodyShape<'graph>> {
        match self.shape {
            Staged::Function(body) => Some(body),
            _ => None,
        }
    }
}

/// The constructor a data member's right-hand side is rooted at, through one-part groups: a list,
/// dict or record literal, or a nominal construction. `None` for anything else.
pub(super) fn root<'graph>(
    shape: &BodyShape<'graph>,
    part: &'graph ExpressionPart<'graph>,
) -> Option<&'graph ExpressionPart<'graph>> {
    match part {
        ExpressionPart::ListLiteral(_)
        | ExpressionPart::DictLiteral(_)
        | ExpressionPart::RecordLiteral(_) => Some(part),
        ExpressionPart::Expression(node) => {
            let node = node.reference();
            match node.parts {
                [only] if node.cache().builtin_shape().is_none() => root(shape, &only.value),
                _ => construction_parts(shape, node).map(|_| part),
            }
        }
        _ => None,
    }
}

/// A nominal construction's head and payload: a formless two-part node whose head is a type name
/// the shape reads.
fn construction_parts<'graph>(
    shape: &BodyShape<'graph>,
    node: &'graph KExpression<'graph>,
) -> Option<(
    &'graph ExpressionPart<'graph>,
    &'graph ExpressionPart<'graph>,
)> {
    match node.parts {
        [head, payload]
            if node.cache().builtin_shape().is_none()
                && matches!(head.value, ExpressionPart::Type(_))
                && shape.mention(Site::of(&head.value)).is_some() =>
        {
            Some((&head.value, &payload.value))
        }
        _ => None,
    }
}

/// The body of `node` when it is a callable that captures a fellow member — a node of the knot
/// being tied. A capture is a `Member` source exactly when it names a fellow of the component its
/// statement's binder sits in, which is the component the walk is tying.
fn fellow_lambda<'graph>(
    shape: &BodyShape<'graph>,
    node: &KExpression<'graph>,
) -> Option<&'graph BodyShape<'graph>> {
    let body = shape.nested(Site::of_body(node)?)?;
    let fellow = body
        .captures()
        .iter()
        .any(|capture| matches!(capture.source, CaptureSource::Member { .. }));
    (body.kind() == ShapeKind::Callable && fellow).then_some(body)
}

/// The walk over data members' right-hand sides.
pub(super) struct Stager<'stage, 'graph, 'cell, 'run> {
    activation: &'stage KActivationView<'graph, 'cell>,
    component: &'stage Component<'graph>,
    /// What an evaluated dict key is read through, should it be a sealed scalar.
    types: &'stage TypeRegistry<'run>,
    scratch: BumpAllocator<'stage>,
    eager: &'stage mut Eager<'stage, 'graph, 'cell>,
    nodes: Nodes<'graph, 'cell, 'stage>,
    /// The member whose right-hand side is being walked.
    owner: u32,
    /// The first part the caller had not evaluated. The walk goes on past it on a placeholder, so
    /// `eager` is asked for every such part in one attempt, and the staging is refused at its end.
    refused: Option<Untieable<'static>>,
}

impl<'stage, 'graph, 'cell, 'run> Stager<'stage, 'graph, 'cell, 'run> {
    /// Stage each member whose root is `Some`, into a node run whose first `roots.len()` indices
    /// are the members.
    ///
    /// Every part only the caller can evaluate is asked of `eager` before the staging is refused on
    /// the first it could not answer, so one refusal names them all by the sites `eager` saw.
    pub(super) fn nodes(
        activation: &'stage KActivationView<'graph, 'cell>,
        component: &'stage Component<'graph>,
        roots: &[Option<&'graph ExpressionPart<'graph>>],
        types: &'stage TypeRegistry<'run>,
        scratch: BumpAllocator<'stage>,
        eager: &'stage mut Eager<'stage, 'graph, 'cell>,
    ) -> Result<Nodes<'graph, 'cell, 'stage>, Untieable<'static>> {
        let mut nodes = BumpVec::with_capacity_in(roots.len(), scratch);
        nodes.extend(roots.iter().map(|_| None));
        let mut stager = Stager {
            activation,
            component,
            types,
            scratch,
            eager,
            nodes,
            owner: 0,
            refused: None,
        };
        let staged = stager.roots(roots);
        // A placeholder can mislead what follows it, so an unevaluated part outranks every later
        // refusal.
        if let Some(refused) = stager.refused {
            return Err(refused);
        }
        staged?;
        Ok(stager.nodes)
    }

    fn roots(
        &mut self,
        roots: &[Option<&'graph ExpressionPart<'graph>>],
    ) -> Result<(), Untieable<'static>> {
        for (index, root) in roots.iter().enumerate() {
            let Some(root) = root else { continue };
            self.owner = index as u32;
            let shape = self.part(root, true)?;
            self.nodes[index] = Some(Node {
                owner: index as u32,
                shape,
            });
        }
        Ok(())
    }

    /// Record that the part at `site` is still the caller's to evaluate, if it is the first.
    fn unevaluated(&mut self, site: Site) {
        let name = self.name();
        self.refused.get_or_insert(Untieable::Eager { name, site });
    }

    fn name(&self) -> BinderSymbol {
        let slot = self.component.members[self.owner as usize];
        self.activation.shape().slot_name(slot)
    }

    /// `part` staged; at a member's `root`, a constructor is returned as the member's own node.
    fn part(
        &mut self,
        part: &'graph ExpressionPart<'graph>,
        root: bool,
    ) -> Result<Staged<'graph, 'cell, 'stage>, Untieable<'static>> {
        let shape = self.activation.shape();
        let scratch = self.scratch;
        let staged = match part {
            ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_) => {
                return Ok(Staged::Literal(part));
            }
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => {
                return match shape.mention(Site::of(part)) {
                    Some(mention) => self.read(mention.name, mention.coordinate),
                    None => self.evaluate(part),
                };
            }
            ExpressionPart::Expression(node) => {
                let node = node.reference();
                if let [only] = node.parts
                    && node.cache().builtin_shape().is_none()
                {
                    return self.part(&only.value, root);
                }
                if let Some(body) = fellow_lambda(shape, node) {
                    let index = self.nodes.len() as u32;
                    self.nodes.push(Some(Node {
                        owner: self.owner,
                        shape: Staged::Function(body),
                    }));
                    return Ok(Staged::Edge(index));
                }
                let Some((head, payload)) = construction_parts(shape, node) else {
                    return self.evaluate(part);
                };
                let mention = shape
                    .mention(Site::of(head))
                    .expect("a construction head is read");
                let head = match self.read(mention.name, mention.coordinate)? {
                    Staged::Value(Value::Type(head)) => head,
                    Staged::Value(other) => {
                        return Err(Untieable::Construction {
                            name: self.name(),
                            site: Site::of(part),
                            refused: ConstructionRefused::NotConstructible(other.ktype()),
                        });
                    }
                    _ => unreachable!(
                        "a type name reads no member of the component: `tie` admits value binders alone"
                    ),
                };
                let payload = self.part(payload, false)?;
                Staged::Tagged {
                    head,
                    site: Site::of(part),
                    payload: scratch.alloc(payload),
                }
            }
            ExpressionPart::ListLiteral(items) => {
                let mut staged = BumpVec::with_capacity_in(items.len(), scratch);
                for item in items.iter() {
                    staged.push(self.part(item, false)?);
                }
                Staged::List(staged)
            }
            ExpressionPart::DictLiteral(pairs) => {
                let mut keys = BumpVec::with_capacity_in(pairs.len(), scratch);
                for (key, _) in pairs.iter() {
                    keys.push((self.key(key)?, ()));
                }
                let mut kept = kept_entries(&keys, scratch);
                kept.sort_unstable();
                let mut entries = BumpVec::with_capacity_in(kept.len(), scratch);
                for at in kept {
                    entries.push((keys[at].0, self.part(&pairs[at].1, false)?));
                }
                Staged::Dict(entries)
            }
            ExpressionPart::RecordLiteral(fields) => {
                let mut staged = BumpVec::with_capacity_in(fields.len(), scratch);
                for (name, value) in fields.iter() {
                    staged.push((*name, self.part(value, false)?));
                }
                Staged::Record(staged)
            }
            ExpressionPart::Keyword(_)
            | ExpressionPart::SigiledTypeExpr(_)
            | ExpressionPart::RecordType(_) => return self.evaluate(part),
        };
        let mut edged = false;
        each_child(&staged, |child| edged |= matches!(child, Staged::Edge(_)));
        if root || !edged {
            return Ok(staged);
        }
        let index = self.nodes.len() as u32;
        self.nodes.push(Some(Node {
            owner: self.owner,
            shape: staged,
        }));
        Ok(Staged::Edge(index))
    }

    /// A mention read: an edge for a member of the component, the activation's word otherwise.
    fn read(
        &self,
        name: BinderSymbol,
        coordinate: Coordinate,
    ) -> Result<Staged<'graph, 'cell, 'stage>, Untieable<'static>> {
        if let Coordinate::Activation {
            hops: 0,
            target: Target::Local(slot),
        } = coordinate
            && let Ok(index) = self.component.members.binary_search(&slot)
        {
            return Ok(Staged::Edge(index as u32));
        }
        let _ = name;
        Ok(Staged::Value(self.activation.read(coordinate)))
    }

    /// A part only the caller can evaluate.
    fn evaluate(
        &mut self,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Staged<'graph, 'cell, 'stage>, Untieable<'static>> {
        let site = Site::of(part);
        match (self.eager)(site, Some(part)) {
            Some(Supplied::Value(value)) => Ok(Staged::Value(value)),
            Some(Supplied::Body(_)) => unreachable!("a data member's part is a value"),
            None => {
                self.unevaluated(site);
                Ok(Staged::Value(Value::Null))
            }
        }
    }

    /// A dict key: a scalar literal as lowering builds it, else the value the caller evaluates.
    fn key(
        &mut self,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Key<'cell>, Untieable<'static>> {
        let site = Site::of(part);
        let key = match part {
            ExpressionPart::Literal(KLiteral::String(literal)) => Ok(Key::str(literal)),
            ExpressionPart::Literal(KLiteral::Number(number)) => Key::number(*number),
            ExpressionPart::Literal(KLiteral::Boolean(flag)) => Ok(Key::bool(*flag)),
            _ => match (self.eager)(site, Some(part)) {
                Some(Supplied::Value(value)) => Key::of(&value, self.types, self.scratch),
                Some(Supplied::Body(_)) => unreachable!("a dict key is a value"),
                None => {
                    self.unevaluated(site);
                    return Ok(Key::bool(false));
                }
            },
        };
        key.map_err(|rejected| Untieable::Key {
            name: self.name(),
            site,
            rejected,
        })
    }
}

/// Visit a staged constructor's direct children: list items, dict and record values, a payload.
fn each_child<'a, 'graph, 'cell, 'x>(
    staged: &'a Staged<'graph, 'cell, 'x>,
    mut visit: impl FnMut(&'a Staged<'graph, 'cell, 'x>),
) {
    match staged {
        Staged::List(items) => items.iter().for_each(visit),
        Staged::Dict(entries) => entries.iter().for_each(|(_, value)| visit(value)),
        Staged::Record(fields) => fields.iter().for_each(|(_, value)| visit(value)),
        Staged::Tagged { payload, .. } => visit(payload),
        Staged::Literal(_) | Staged::Value(_) | Staged::Edge(_) | Staged::Function(_) => {}
    }
}

/// Whether a staged node's memo is derived from its cells: a container, or a construction whose
/// head solves its identity from its payload. Any other tagged node is a cut.
fn is_derived(staged: &Staged<'_, '_, '_>, types: &TypeRegistry<'_>) -> bool {
    match staged {
        Staged::List(_) | Staged::Dict(_) | Staged::Record(_) => true,
        Staged::Tagged { head, .. } => solves_identity(types, head.handle()),
        _ => false,
    }
}

/// Every node's memo: `memos[i]` is `Some` for a function node on entry, and on success every
/// node's memo is `Some`. A cycle of derived nodes refuses with the members holding it, and a
/// construction the rule refuses while its memo is derived refuses as it would at the check.
pub(super) fn memos<'x>(
    nodes: &Nodes<'_, '_, '_>,
    memos: &mut [Option<KType>],
    names: &dyn Fn(u32) -> BinderSymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<(), Untieable<'x>> {
    let derived = |node: &Option<Node<'_, '_, '_>>| {
        node.as_ref()
            .is_some_and(|node| is_derived(&node.shape, types))
    };
    for (index, node) in nodes.iter().enumerate() {
        if let Some(Node {
            shape: Staged::Tagged { head, .. },
            ..
        }) = node
            && !derived(node)
        {
            memos[index] = Some(head.handle());
        }
    }
    let mut rows: BumpVec<'x, BumpVec<'x, usize>> = BumpVec::with_capacity_in(nodes.len(), scratch);
    for node in nodes.iter() {
        let mut row = BumpVec::new_in(scratch);
        if let Some(node) = node.as_ref().filter(|node| is_derived(&node.shape, types)) {
            each_child(&node.shape, |child| {
                if let Staged::Edge(target) = child
                    && derived(&nodes[*target as usize])
                {
                    row.push(*target as usize);
                }
            });
        }
        rows.push(row);
    }
    let mut edges: BumpVec<'x, &[usize]> = BumpVec::with_capacity_in(rows.len(), scratch);
    edges.extend(rows.iter().map(|row| row.as_slice()));
    for component in strongly_connected_components(scratch, &edges) {
        let first = component[0];
        if component.len() > 1 || edges[first].contains(&first) {
            let mut owners: BumpVec<'x, u32> = BumpVec::with_capacity_in(component.len(), scratch);
            owners.extend(component.iter().map(|node| {
                nodes[*node]
                    .as_ref()
                    .expect("a derived node is a data node")
                    .owner
            }));
            owners.sort_unstable();
            owners.dedup();
            let mut listed = BumpVec::with_capacity_in(owners.len(), scratch);
            listed.extend(owners.iter().map(|owner| names(*owner)));
            return Err(Untieable::TypeCycle {
                names: scratch.alloc_slice_copy(&listed),
            });
        }
        if let Some(node) = nodes[first]
            .as_ref()
            .filter(|node| is_derived(&node.shape, types))
        {
            let memo =
                staged_type(&node.shape, memos, types, scratch).map_err(|(site, refused)| {
                    Untieable::Construction {
                        name: names(node.owner),
                        site,
                        refused,
                    }
                })?;
            memos[first] = Some(memo);
        }
    }
    Ok(())
}

/// The type a staged part's value memoizes, by the rule the plain door of its kind derives its memo
/// by — so a nested construction checked here is the one [`write`] builds — an edge its target's
/// memo, which is derived first. A construction through a family takes the application the rule
/// solves, and one the rule refuses comes back with its site.
fn staged_type(
    staged: &Staged<'_, '_, '_>,
    memos: &[Option<KType>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, (Site, ConstructionRefused)> {
    let of = |staged| staged_type(staged, memos, types, scratch);
    Ok(match staged {
        Staged::Literal(part) => part_ktype(part, types, scratch).expect("a literal has a type"),
        Staged::Value(value) => value.ktype(),
        Staged::Edge(target) => {
            memos[*target as usize].expect("a referent's memo is derived first")
        }
        Staged::List(items) => {
            let mut cells = BumpVec::with_capacity_in(items.len(), scratch);
            for item in items.iter() {
                cells.push(of(item)?);
            }
            list_type(types, scratch, cells.iter().copied())
        }
        Staged::Dict(entries) => {
            let mut cells = BumpVec::with_capacity_in(entries.len(), scratch);
            for (key, value) in entries.iter() {
                cells.push((key.ktype(), of(value)?));
            }
            dict_type(types, scratch, cells.iter().copied())
        }
        Staged::Record(fields) => {
            let mut cells = BumpVec::with_capacity_in(fields.len(), scratch);
            for (name, value) in fields.iter() {
                cells.push((*name, of(value)?));
            }
            record_type(types, scratch, cells.iter().copied())
        }
        Staged::Tagged {
            head,
            site,
            payload,
        } if solves_identity(types, head.handle()) => {
            construction(types, scratch, head.handle(), of(payload)?)
                .map_err(|refused| (*site, refused))?
        }
        Staged::Tagged { head, .. } => head.handle(),
        Staged::Function(_) => {
            unreachable!("a function sits only at a node, reached through an edge")
        }
    })
}

/// Check every construction the nodes hold — a tagged node over its payload's memo, and every
/// ordinary construction below — in node order, depth first.
pub(super) fn check(
    nodes: &Nodes<'_, '_, '_>,
    memos: &[Option<KType>],
    names: &dyn Fn(u32) -> BinderSymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), Untieable<'static>> {
    fn walk(
        staged: &Staged<'_, '_, '_>,
        memos: &[Option<KType>],
        name: BinderSymbol,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<(), Untieable<'static>> {
        if let Staged::Tagged {
            head,
            site,
            payload,
        } = staged
        {
            let refusal = |(site, refused)| Untieable::Construction {
                name,
                site,
                refused,
            };
            let payload = staged_type(payload, memos, types, scratch).map_err(refusal)?;
            construction(types, scratch, head.handle(), payload)
                .map_err(|refused| refusal((*site, refused)))?;
        }
        let mut refused = Ok(());
        each_child(staged, |child| {
            if refused.is_ok() && !matches!(child, Staged::Edge(_)) {
                refused = walk(child, memos, name, types, scratch);
            }
        });
        refused
    }
    for node in nodes.iter().flatten() {
        walk(&node.shape, memos, names(node.owner), types, scratch)?;
    }
    Ok(())
}

/// A checked staged part laid down as a value in `writer`'s region.
fn write<'graph, 'cell>(
    writer: Writer<'cell>,
    staged: &Staged<'graph, 'cell, '_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> KValue<'graph, 'cell> {
    let value = |staged| write(writer, staged, types, scratch);
    match staged {
        Staged::Literal(part) => {
            Value::lower_part(writer, part, types, scratch).expect("a literal lowers")
        }
        Staged::Value(value) => *value,
        Staged::Edge(_) => unreachable!("an edge sits only directly in a node"),
        Staged::List(items) => {
            let mut cells = BumpVec::with_capacity_in(items.len(), scratch);
            cells.extend(items.iter().map(value));
            Value::List(List::new(writer, cells.iter().copied(), types, scratch))
        }
        Staged::Dict(entries) => {
            let mut cells = BumpVec::with_capacity_in(entries.len(), scratch);
            cells.extend(entries.iter().map(|(key, cell)| (*key, value(cell))));
            Value::Dict(Dict::new(writer, &cells, types, scratch))
        }
        Staged::Record(fields) => {
            let mut cells = BumpVec::with_capacity_in(fields.len(), scratch);
            cells.extend(fields.iter().map(|(name, cell)| (*name, value(cell))));
            Value::Record(Record::new(writer, &cells, types, scratch))
        }
        Staged::Tagged { head, payload, .. } => Value::Tagged(
            Tagged::construct(writer, head, value(payload), types, scratch)
                .expect("every construction is checked before the first write"),
        ),
        Staged::Function(_) => {
            unreachable!("a function sits only at a node, reached through an edge")
        }
    }
}

/// A node's cell: an edge through `plan`, or a staged value written.
fn link<'graph, 'cell>(
    writer: Writer<'cell>,
    staged: &Staged<'graph, 'cell, '_>,
    plan: &KnotPlan,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Link<'graph, 'cell, Knotted<'graph, 'cell>> {
    match staged {
        Staged::Edge(target) => Link::Edge(
            plan.edge(*target)
                .expect("a node index is below the knot's count"),
        ),
        other => Link::Value(write(writer, other, types, scratch)),
    }
}

/// A checked data node laid down in `writer`'s region under its memo.
pub(super) fn lay_down<'graph, 'cell>(
    writer: Writer<'cell>,
    node: &Node<'graph, 'cell, '_>,
    memo: KType,
    plan: &KnotPlan,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Circular<'graph, 'cell, Knotted<'graph, 'cell>> {
    let cell = |staged| link(writer, staged, plan, types, scratch);
    match &node.shape {
        Staged::List(items) => {
            let mut cells = BumpVec::with_capacity_in(items.len(), scratch);
            cells.extend(items.iter().map(cell));
            Circular::List(List::linked(writer, &cells, memo))
        }
        Staged::Dict(entries) => {
            let mut cells = BumpVec::with_capacity_in(entries.len(), scratch);
            cells.extend(entries.iter().map(|(key, staged)| (*key, cell(staged))));
            Circular::Dict(Dict::linked(writer, &cells, memo, scratch))
        }
        Staged::Record(fields) => {
            let mut cells = BumpVec::with_capacity_in(fields.len(), scratch);
            cells.extend(fields.iter().map(|(name, staged)| (*name, cell(staged))));
            Circular::Record(Record::linked(writer, &cells, memo, scratch))
        }
        Staged::Tagged { payload, .. } => {
            Circular::Tagged(Tagged::linked(writer, cell(payload), memo))
        }
        Staged::Literal(_) | Staged::Value(_) | Staged::Edge(_) | Staged::Function(_) => {
            unreachable!("a data node is a constructor")
        }
    }
}
