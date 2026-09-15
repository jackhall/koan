//! Shared scaffolding for `values`' suites: program storage with a registry built in it, a scratch
//! and an interner, a graph over that storage to run steps in, and a test-only knot member whose
//! every node is a data node, with a sealed singleton newtype to tag its rings.

mod boundary;
mod construction;
mod crossing;
mod equality;
mod render;
mod satisfaction;
mod working;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, Edge, KnotPlan, Member, Prices, ProgramBrand,
    ReleaseAbsorption, StepContext, Verdict, Writer, program_storage, reattachable,
};
use crate::parse::{ExpressionPart, KExpression, LabelInterner, TypeSymbol, parse};
use crate::type_lattice::{KType, RecursiveGroupWindow, RelativeSchema, TypeRegistry};
use crate::values::{Circular, DeepCopy, Knotted, KnottedFamily, Resolved, Weight};

/// A value holding no callable — what every suite here builds, spelled once so a literal arm
/// needs no annotation. The containers and the working form follow it.
pub(super) type Value<'graph, 'cell> = crate::values::Value<'graph, 'cell>;
pub(super) type List<'graph, 'cell> = crate::values::List<'graph, 'cell>;
pub(super) type Dict<'graph, 'cell> = crate::values::Dict<'graph, 'cell>;
pub(super) type Record<'graph, 'cell> = crate::values::Record<'graph, 'cell>;
pub(super) type Tagged<'graph, 'cell> = crate::values::Tagged<'graph, 'cell>;
pub(super) type WorkingExpression<'graph, 'cell> = crate::values::WorkingExpression<'graph, 'cell>;
pub(super) type WorkingPart<'graph, 'cell> = crate::values::WorkingPart<'graph, 'cell>;

/// [`crate::values::text`] at [`Value`].
pub(super) fn text<'graph, 'cell>(
    writer: crate::memory::Writer<'cell>,
    text: &str,
) -> Value<'graph, 'cell> {
    crate::values::text(writer, text)
}

/// A continuation family for a graph whose cells only store.
pub(super) struct Step;
reattachable!(Step => ());

/// What a test reads and writes outside the graph. `'graph` is the program storage the parsed AST
/// and the registry live in, which a graph built over it borrows.
pub(super) struct Fixture<'f, 'graph> {
    pub program: ProgramBrand<'graph>,
    pub types: &'f TypeRegistry<'graph>,
    pub labels: &'f LabelInterner,
    scratch: &'f Bump,
}

impl<'graph> Fixture<'_, 'graph> {
    pub fn scratch(&self) -> BumpAllocator<'_> {
        self.scratch
    }

    /// The first top-level expression of `source`, parsed into program storage.
    pub fn parse(&self, source: &str) -> KExpression<'graph> {
        parse(self.program, self.labels, source)
            .expect("the test source parses")
            .into_iter()
            .next()
            .expect("the test source has a line")
    }

    /// The single part `source` parses to.
    pub fn part(&self, source: &str) -> ExpressionPart<'graph> {
        match self.parse(source).parts {
            [only] => only.value,
            parts => panic!("`{source}` parsed to {} parts", parts.len()),
        }
    }

    /// Run `step` in one cell of a fresh graph over this fixture's storage, under `verdict`.
    pub fn in_cell<R>(
        &self,
        verdict: fn(Prices) -> Verdict,
        step: impl for<'step, 'here> FnOnce(&mut StepContext<'graph, 'step, 'here, Step>) -> R,
    ) -> R {
        let mut graph: CellGraph<'graph, Step> = CellGraph::new(1, verdict);
        let cell = graph
            .create(None, None)
            .expect("a one-slot graph has a free slot");
        let out = graph.enter(cell, step).expect("a fresh cell is enterable");
        graph
            .release(cell, ReleaseAbsorption::IntoHolder)
            .expect("a cell outside its step releases");
        out
    }
}

/// Run `test` against a fresh fixture.
pub(super) fn with_fixture<R>(test: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>) -> R) -> R {
    let storage = program_storage();
    let program = storage.brand();
    let types = TypeRegistry::in_region(program.allocator());
    let labels = LabelInterner::new();
    let scratch = Bump::new();
    test(&Fixture {
        program,
        types: &types,
        labels: &labels,
        scratch: &scratch,
    })
}

/// Pin every operand.
pub(super) fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

/// Copy every operand.
pub(super) fn copy(_: Prices) -> Verdict {
    Verdict::Copy
}

/// A test-only knot member: every node is a data node, and its memo is the node's own.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Node<'graph, 'cell>(Member<'cell, Circular<'graph, 'cell, Node<'graph, 'cell>>>);

impl Knotted for Node<'_, '_> {
    fn ktype(&self) -> KType {
        self.0.payload().ktype()
    }

    fn weight(&self) -> Weight {
        Weight::ZERO
    }

    fn sibling(&self, edge: Edge) -> Self {
        Node(self.0.follow(edge))
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self>
    where
        Self: 'a,
    {
        Resolved::Circular(*self.0.payload())
    }
}

impl std::fmt::Debug for Node<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node({})", self.0.index().index())
    }
}

/// The family of [`Node`]: a copy re-ties the whole knot, each node rebuilt in index order.
pub(super) struct NodeFamily;

impl<'graph> KnottedFamily<'graph> for NodeFamily {
    type Closed<'cell>
        = Node<'graph, 'cell>
    where
        'graph: 'cell;

    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        member: &Node<'graph, 'from>,
        copy: &mut DeepCopy<'_, 'graph, 'from, 'to, Node<'graph, 'from>, Node<'graph, 'to>>,
    ) -> Node<'graph, 'to>
    where
        'graph: 'from,
        'graph: 'to,
    {
        let source = member.0.knot();
        let knot = KnotPlan::new(source.len()).tie(writer, |edge| {
            source.member(edge).payload().copied(writer, copy)
        });
        Node(knot.member(member.0.index()))
    }
}

/// A value that may hold a [`Node`].
pub(super) type Holding<'graph, 'cell> = crate::values::Value<'graph, 'cell, Node<'graph, 'cell>>;

/// A link a [`Node`]'s cell holds.
pub(super) type Link<'graph, 'cell> = crate::values::Link<'graph, 'cell, Node<'graph, 'cell>>;

/// Tie `count` nodes in `writer`'s region: `build` is handed each node's index and every node's
/// edge, and returns its finished payload. Every node, in index order.
pub(super) fn tie<'graph, 'cell>(
    writer: Writer<'cell>,
    count: u32,
    mut build: impl FnMut(u32, &[Edge]) -> Circular<'graph, 'cell, Node<'graph, 'cell>>,
) -> Vec<Node<'graph, 'cell>> {
    let plan = KnotPlan::new(count);
    let edges: Vec<Edge> = (0..count).map(|index| plan.edge(index).unwrap()).collect();
    let knot = plan.tie(writer, |edge| build(edge.index(), &edges));
    knot.members().map(Node).collect()
}

impl<'graph> Fixture<'_, 'graph> {
    /// `NEWTYPE <name> = :{<field> :<name>}`, sealed as a singleton recursive group.
    pub fn ring_type(&self, name: &str, field: &str) -> KType {
        let (types, scratch) = (self.types, self.scratch());
        let field = crate::parse::BinderSymbol::declared(field, self.labels).unwrap();
        let representation = types.record(scratch, &[(field, types.sibling(0))]);
        RecursiveGroupWindow::seal_singleton(
            scratch,
            TypeSymbol::declared(name, self.labels).unwrap(),
            RelativeSchema::NewType(representation),
            None,
            types,
            scratch,
        )
    }

    /// `NEWTYPE <name> = <representation>`, sealed as a singleton recursive group.
    pub fn newtype(&self, name: &str, representation: KType) -> KType {
        RecursiveGroupWindow::seal_singleton(
            self.scratch(),
            TypeSymbol::declared(name, self.labels).unwrap(),
            RelativeSchema::NewType(representation),
            None,
            self.types,
            self.scratch(),
        )
    }
}

/// A ring of tagged members, member `i` tagged `identity` around a record node whose `next` is an
/// edge to member `i + 1` (wrapping), beside a `value` field when `values[i]` is given. Member `i`
/// is node `2i` and its record node `2i + 1`; the members come back in order.
pub(super) fn ring<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    writer: Writer<'cell>,
    identity: KType,
    values: &[Option<Holding<'graph, 'cell>>],
) -> Vec<Node<'graph, 'cell>> {
    let (types, scratch, labels) = (fixture.types, fixture.scratch(), fixture.labels);
    let next = crate::parse::BinderSymbol::declared("next", labels).unwrap();
    let value = crate::parse::BinderSymbol::declared("value", labels).unwrap();
    let count = values.len() as u32;
    let nodes = tie(writer, 2 * count, |index, edges| {
        let member = (index / 2) as usize;
        if index % 2 == 0 {
            return Circular::Tagged(crate::values::Tagged::linked(
                writer,
                Link::Edge(edges[index as usize + 1]),
                identity,
            ));
        }
        let successor = Link::Edge(edges[2 * ((member + 1) % values.len())]);
        let (fields, memo): (Vec<_>, _) = match values[member] {
            Some(cell) => (
                vec![(next, successor), (value, Link::Value(cell))],
                types.record(scratch, &[(next, identity), (value, cell.ktype())]),
            ),
            None => (
                vec![(next, successor)],
                types.record(scratch, &[(next, identity)]),
            ),
        };
        Circular::Record(crate::values::Record::linked(
            writer, &fields, memo, scratch,
        ))
    });
    nodes.into_iter().step_by(2).collect()
}
