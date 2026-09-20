//! A knot's data members: a `LET` whose right-hand side, through one-part groups, is a list, dict or
//! record literal or a nominal construction `(Head payload)`.
//!
//! **Staging** reads a member's right-hand side into scratch with no writer in reach. A literal waits
//! to be lowered; a mention of a fellow member is an edge; any other mention is the word the
//! activation reads, or a refusal while its binder runs; a part the walk cannot build itself — a
//! call, a keyword form, a `FN` — is asked of the caller's evaluator by site. Every constructor on
//! the path from a member's root to a fellow mention is an anonymous node of the same knot, indexed
//! after the members in the order the walk meets it, and the cell that held it holds an edge; a
//! nested constructor with no fellow mention below it stays an ordinary value.
//!
//! **Memos** follow the nominal cut. A function's memo is its signature type and a tagged node's is
//! the newtype its head names, both known before the knot exists; a container node's memo is what
//! the plain door of its kind would memoize, an edge contributing its target's memo. So container
//! memos are derived in reverse topological order over the edges between container nodes, and a
//! cycle of containers alone has no finite type and refuses the tie.
//!
//! **Constructions** are checked by [`values::construction`](crate::values::construction), the one
//! rule an ordinary construction goes through too, before anything is written.

use crate::memory::{BumpAllocator, BumpVec, KnotPlan, Writer, strongly_connected_components};
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::scope::{Binding, BodyShape, Component, Coordinate, Site, Target};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{
    Circular, ConstructionRefused, Dict, Key, Link, List, Record, Tagged, TypeValue, Value,
    construction, dict_type, kept_entries, list_type, part_ktype, record_type,
};

use super::{KActivation, KValue, Knotted, Supplied, Untieable};

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
}

/// A staged data node: the member whose right-hand side holds it, and its constructor.
pub(super) struct Node<'graph, 'cell, 'x> {
    pub owner: u32,
    pub shape: Staged<'graph, 'cell, 'x>,
}

/// Every knot node by index: a data node staged, or `None` for a function member.
pub(super) type Nodes<'graph, 'cell, 'x> = BumpVec<'x, Option<Node<'graph, 'cell, 'x>>>;

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

/// The walk over data members' right-hand sides.
pub(super) struct Stager<'stage, 'graph, 'cell> {
    activation: &'stage KActivation<'graph, 'cell>,
    component: &'stage Component<'graph>,
    scratch: BumpAllocator<'stage>,
    eager: &'stage mut dyn FnMut(Site) -> Option<Supplied<'graph, 'cell>>,
    nodes: Nodes<'graph, 'cell, 'stage>,
    /// The member whose right-hand side is being walked.
    owner: u32,
}

impl<'stage, 'graph, 'cell> Stager<'stage, 'graph, 'cell> {
    /// Stage each member whose root is `Some`, into a node run whose first `roots.len()` indices
    /// are the members.
    pub(super) fn nodes(
        activation: &'stage KActivation<'graph, 'cell>,
        component: &'stage Component<'graph>,
        roots: &[Option<&'graph ExpressionPart<'graph>>],
        scratch: BumpAllocator<'stage>,
        eager: &'stage mut dyn FnMut(Site) -> Option<Supplied<'graph, 'cell>>,
    ) -> Result<Nodes<'graph, 'cell, 'stage>, Untieable<'static>> {
        let mut nodes = BumpVec::with_capacity_in(roots.len(), scratch);
        nodes.extend(roots.iter().map(|_| None));
        let mut stager = Stager {
            activation,
            component,
            scratch,
            eager,
            nodes,
            owner: 0,
        };
        for (index, root) in roots.iter().enumerate() {
            let Some(root) = root else { continue };
            stager.owner = index as u32;
            let shape = stager.part(root, true)?;
            stager.nodes[index] = Some(Node {
                owner: index as u32,
                shape,
            });
        }
        Ok(stager.nodes)
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
                            refused: ConstructionRefused::NotNewType(other.ktype()),
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
        match self.activation.read(coordinate) {
            Binding::Bound(value) => Ok(Staged::Value(value)),
            Binding::Pending(binder) => Err(Untieable::Pending { name, binder }),
        }
    }

    /// A part only the caller can evaluate.
    fn evaluate(
        &mut self,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Staged<'graph, 'cell, 'stage>, Untieable<'static>> {
        let site = Site::of(part);
        match (self.eager)(site) {
            Some(Supplied::Value(value)) => Ok(Staged::Value(value)),
            Some(Supplied::Body(_)) => unreachable!("a data member's part is a value"),
            None => Err(Untieable::Eager {
                name: self.name(),
                site,
            }),
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
            _ => match (self.eager)(site) {
                Some(Supplied::Value(value)) => Key::of(&value),
                Some(Supplied::Body(_)) => unreachable!("a dict key is a value"),
                None => {
                    return Err(Untieable::Eager {
                        name: self.name(),
                        site,
                    });
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
        Staged::Literal(_) | Staged::Value(_) | Staged::Edge(_) => {}
    }
}

/// Whether a staged node is a container, whose memo is derived from its cells.
fn is_container(staged: &Staged<'_, '_, '_>) -> bool {
    matches!(
        staged,
        Staged::List(_) | Staged::Dict(_) | Staged::Record(_)
    )
}

/// Every node's memo: `memos[i]` is `Some` for a function member on entry, and on success every
/// node's memo is `Some`. A cycle of container nodes refuses with the members holding it.
pub(super) fn memos<'x>(
    nodes: &Nodes<'_, '_, '_>,
    memos: &mut [Option<KType>],
    names: &dyn Fn(u32) -> BinderSymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<(), Untieable<'x>> {
    for (index, node) in nodes.iter().enumerate() {
        if let Some(Node {
            shape: Staged::Tagged { head, .. },
            ..
        }) = node
        {
            memos[index] = Some(head.handle());
        }
    }
    let mut rows: BumpVec<'x, BumpVec<'x, usize>> = BumpVec::with_capacity_in(nodes.len(), scratch);
    for node in nodes.iter() {
        let mut row = BumpVec::new_in(scratch);
        if let Some(node) = node.as_ref().filter(|node| is_container(&node.shape)) {
            each_child(&node.shape, |child| {
                if let Staged::Edge(target) = child
                    && nodes[*target as usize]
                        .as_ref()
                        .is_some_and(|target| is_container(&target.shape))
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
                    .expect("a container is a data node")
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
            .filter(|node| is_container(&node.shape))
        {
            memos[first] = Some(staged_type(&node.shape, memos, types, scratch));
        }
    }
    Ok(())
}

/// The type a staged part's value memoizes, by the rule the plain door of its kind derives its memo
/// by — so a nested construction checked here is the one [`write`] builds — an edge its target's
/// memo, which is derived first.
fn staged_type(
    staged: &Staged<'_, '_, '_>,
    memos: &[Option<KType>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    let of = |staged| staged_type(staged, memos, types, scratch);
    match staged {
        Staged::Literal(part) => part_ktype(part, types, scratch).expect("a literal has a type"),
        Staged::Value(value) => value.ktype(),
        Staged::Edge(target) => {
            memos[*target as usize].expect("a referent's memo is derived first")
        }
        Staged::List(items) => list_type(types, scratch, items.iter().map(of)),
        Staged::Dict(entries) => dict_type(
            types,
            scratch,
            entries.iter().map(|(key, value)| (key.ktype(), of(value))),
        ),
        Staged::Record(fields) => record_type(
            types,
            scratch,
            fields.iter().map(|(name, value)| (*name, of(value))),
        ),
        Staged::Tagged { head, .. } => head.handle(),
    }
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
            let payload = staged_type(payload, memos, types, scratch);
            construction(types, scratch, head.handle(), payload).map_err(|refused| {
                Untieable::Construction {
                    name,
                    site: *site,
                    refused,
                }
            })?;
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
        Staged::Literal(_) | Staged::Value(_) | Staged::Edge(_) => {
            unreachable!("a data node is a constructor")
        }
    }
}
