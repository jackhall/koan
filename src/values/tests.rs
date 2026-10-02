//! Shared scaffolding for `values`' suites: program storage with a registry built in it, a scratch
//! and an interner, a graph over that storage to run steps in, and a test-only knot member whose
//! every node is a data node, with a sealed singleton newtype to tag its rings. [`parts`] is shared
//! crate-wide: a suite outside `values` reads a value's parts through the door as a program does.

mod boundary;
mod construction;
mod crossing;
mod depth;
mod equality;
mod render;
mod satisfaction;
mod surface;
mod working;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, Edge, KnotPlan, Member, Prices, ProgramBrand,
    ReleaseAbsorption, StepContext, Verdict, Writer, covariant, program_storage, reattachable,
};
use crate::parse::{ExpressionPart, KExpression, ProgramNode, parse};
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, RecursiveGroupWindow, RelativeSchema, TypeRegistry,
};
use crate::values::{Circular, CodeView, DeepCopy, Knotted, KnottedFamily, Resolved, Weight};

/// A value holding no callable — what every suite here builds, spelled once so a literal arm
/// needs no annotation. The containers and the working form follow it.
pub(super) type Value<'cell> = crate::values::Value<'cell>;
pub(super) type List<'cell> = crate::values::List<'cell>;
pub(super) type Dict<'cell> = crate::values::Dict<'cell>;
pub(super) type Record<'cell> = crate::values::Record<'cell>;
pub(super) type Tagged<'cell> = crate::values::Tagged<'cell>;
pub(super) type WorkingExpression<'graph, 'cell> = crate::values::WorkingExpression<'graph, 'cell>;
pub(super) type WorkingPart<'graph, 'cell> = crate::values::WorkingPart<'graph, 'cell>;

/// [`crate::values::text`] at [`Value`].
pub(super) fn text<'cell>(writer: crate::memory::Writer<'cell>, text: &str) -> Value<'cell> {
    crate::values::text(writer, text)
}

/// The parts `value` shows through [the door](crate::values::Surface), each at its own memo: a
/// list's elements, a dict's values in key order, a record's fields in symbol order, a tagged
/// value's payload. Empty for a value that does not open.
pub(crate) fn parts<'cell, X: Knotted + 'cell>(
    value: crate::values::Value<'cell, X>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Vec<crate::values::Value<'cell, X>> {
    let Some(surface) = value.surface(types, scratch) else {
        return Vec::new();
    };
    (0..surface.len())
        .map(|at| surface.child(at, types, scratch).value())
        .collect()
}

/// A dict's cell under `key`, read off its runs as the door never shows a reader outside `values`.
pub(super) fn entry<'cell, X: Copy>(
    dict: &crate::values::Dict<'cell, X>,
    key: &crate::values::Key<'_>,
) -> Option<&'cell crate::values::Value<'cell, X>> {
    let at = dict.keys().binary_search_by(|probe| probe.cmp(key)).ok()?;
    dict.cells().get(at)
}

/// A record's cell under `name`, read off its runs whatever its type shows.
pub(super) fn held<'cell, X: Copy>(
    record: &crate::values::Record<'cell, X>,
    name: crate::symbols::Symbol,
) -> Option<&'cell crate::values::Value<'cell, X>> {
    let at = record.names().binary_search(&name).ok()?;
    record.cells().get(at)
}

/// A continuation family for a graph whose cells only store.
pub(super) struct Step;
reattachable!(Step => ());

/// What a test reads and writes outside the graph. `'graph` is the program storage the parsed AST
/// and the registry live in, which a graph built over it borrows.
pub(super) struct Fixture<'f, 'graph> {
    pub program: ProgramBrand<'graph>,
    pub types: &'f TypeRegistry<'graph>,
    pub symbols: &'f SymbolInterner,
    scratch: &'f Bump,
}

impl<'graph> Fixture<'_, 'graph> {
    pub fn scratch(&self) -> BumpAllocator<'_> {
        self.scratch
    }

    /// The first top-level expression of `source`, parsed into program storage.
    pub fn parse(&self, source: &str) -> KExpression<'graph> {
        parse(self.program, self.symbols, source)
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
        step: impl for<'step, 'here, 'scratch> FnOnce(
            &mut StepContext<'graph, 'step, 'here, 'scratch, Step>,
        ) -> R,
    ) -> R {
        let mut graph: CellGraph<'graph, Step> = CellGraph::new(1, verdict);
        let cell = graph
            .create(None)
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
    // The lattice's registry is a collections arena, so it keeps a bump of its own; program
    // storage is the one `cellgraph` store everything else here is written into.
    let registry = Bump::new();
    let types = TypeRegistry::in_region(&registry);
    let symbols = SymbolInterner::new();
    let scratch = Bump::new();
    test(&Fixture {
        program,
        types: &types,
        symbols: &symbols,
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

/// A stand-in for a knot member only a layer above `values` builds: a function of an identity that
/// captures nothing, a barrier, or a quote's code that binds no name. Equal and hashed by that
/// identity, a quote's by its body's address.
#[derive(Clone, Copy, Debug)]
pub(super) enum Stand<'graph> {
    Function(usize),
    Barrier,
    Code(ProgramNode<'graph>),
}

impl Stand<'_> {
    fn identity(&self) -> (u8, usize) {
        match self {
            Stand::Function(identity) => (0, *identity),
            Stand::Barrier => (1, 0),
            Stand::Code(node) => (2, std::ptr::from_ref(node.reference()).addr()),
        }
    }
}

impl PartialEq for Stand<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}

impl Eq for Stand<'_> {}

impl std::hash::Hash for Stand<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.identity().hash(state);
    }
}

impl Knotted for Stand<'_> {
    fn ktype(&self) -> DeclaredType<KType> {
        match self {
            Stand::Code(node) => node.code_kind().into(),
            Stand::Function(_) | Stand::Barrier => KType::ANY.into(),
        }
    }

    fn weight(&self) -> Weight {
        Weight::ZERO
    }

    fn sibling(&self, _: Edge) -> Self {
        *self
    }

    /// A stand-in is a knot of one.
    fn index(&self) -> Edge {
        KnotPlan::new(1).edge(0).expect("a knot of one has node 0")
    }

    fn root(&self) -> Self {
        *self
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self>
    where
        Self: 'a,
    {
        match *self {
            Stand::Function(identity) => Resolved::Function {
                identity,
                instance: &[],
                closure: &[],
            },
            Stand::Barrier => Resolved::Barrier,
            Stand::Code(node) => Resolved::Code(CodeView {
                body: node.reference(),
                bound: &[],
                supplied: &[],
            }),
        }
    }
}

/// The quote `source` parses to.
pub(super) fn quote<'graph>(fixture: &Fixture<'_, 'graph>, source: &str) -> ProgramNode<'graph> {
    match fixture.part(source) {
        ExpressionPart::QuotedExpression(node) => node,
        _ => panic!("`{source}` parses to a quote"),
    }
}

/// A test-only knot member: every node is a data node, and its memo is the node's own.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Node<'cell>(Member<'cell, Circular<'cell, Node<'cell>>>);

impl Knotted for Node<'_> {
    fn ktype(&self) -> DeclaredType<KType> {
        self.0.payload().ktype().into()
    }

    fn weight(&self) -> Weight {
        Weight::ZERO
    }

    fn sibling(&self, edge: Edge) -> Self {
        Node(self.0.follow(edge))
    }

    fn index(&self) -> Edge {
        self.0.index()
    }

    fn root(&self) -> Self {
        Node(self.0.knot().members().next().expect("a knot holds a node"))
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self>
    where
        Self: 'a,
    {
        Resolved::Circular(*self.0.payload())
    }
}

impl std::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node({})", self.0.index().index())
    }
}

/// The family of [`Node`]: a copy re-ties the whole knot, each node rebuilt in index order.
pub(super) struct NodeFamily;

impl<'graph> KnottedFamily<'graph> for NodeFamily {
    type Closed<'cell>
        = Node<'cell>
    where
        'graph: 'cell;

    fn held<'from>(member: &Node<'from>, out: &mut dyn FnMut(Holding<'from>))
    where
        'graph: 'from,
    {
        for node in member.0.knot().members() {
            node.payload().held(out);
        }
    }

    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        member: &Node<'from>,
        copy: &mut DeepCopy<'_, 'from, 'to, Node<'from>, Node<'to>>,
    ) -> Node<'to>
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

covariant!(crate::values::ValueFamily<NodeFamily>);

/// A value that may hold a [`Node`].
pub(super) type Holding<'cell> = crate::values::Value<'cell, Node<'cell>>;

/// A link a [`Node`]'s cell holds.
pub(super) type Link<'cell> = crate::values::Link<'cell, Node<'cell>>;

/// Tie `count` nodes in `writer`'s region: `build` is handed each node's index and every node's
/// edge, and returns its finished payload. Every node, in index order.
pub(super) fn tie<'graph, 'cell>(
    writer: Writer<'cell>,
    count: u32,
    mut build: impl FnMut(u32, &[Edge]) -> Circular<'cell, Node<'cell>>,
) -> Vec<Node<'cell>> {
    let plan = KnotPlan::new(count);
    let edges: Vec<Edge> = (0..count).map(|index| plan.edge(index).unwrap()).collect();
    let knot = plan.tie(writer, |edge| build(edge.index(), &edges));
    knot.members().map(Node).collect()
}

impl<'graph> Fixture<'_, 'graph> {
    /// `NEWTYPE <name> = :{<field> :<name>}`, sealed as a singleton recursive group.
    pub fn ring_type(&self, name: &str, field: &str) -> KType {
        let (types, scratch) = (self.types, self.scratch());
        let field = crate::symbols::BinderSymbol::declared(field, self.symbols).unwrap();
        let representation = types.record(scratch, &[(field, types.sibling(0))]);
        RecursiveGroupWindow::seal_singleton(
            scratch,
            TypeSymbol::declared(name, self.symbols).unwrap(),
            RelativeSchema::NewType(representation),
            None,
            types,
            scratch,
        )
    }

    /// A family `name` over `params`, sealed as a singleton recursive group: `representation` is
    /// handed the declared names symbol-sorted, the order its quantifiers index, and answers the
    /// representation a construction wraps.
    pub fn family(
        &self,
        name: &str,
        params: &[&str],
        representation: impl FnOnce(&[TypeSymbol]) -> Option<Parametric>,
    ) -> KType {
        let mut names: Vec<TypeSymbol> = params
            .iter()
            .map(|param| TypeSymbol::declared(param, self.symbols).unwrap())
            .collect();
        names.sort_unstable();
        let schema = RelativeSchema::constructor(
            self.scratch(),
            self.scratch(),
            representation(&names),
            &names,
        );
        RecursiveGroupWindow::seal_singleton(
            self.scratch(),
            TypeSymbol::declared(name, self.symbols).unwrap(),
            schema,
            None,
            self.types,
            self.scratch(),
        )
    }

    /// `NEWTYPE <name> = <representation>`, sealed as a singleton recursive group.
    pub fn newtype(&self, name: &str, representation: KType) -> KType {
        RecursiveGroupWindow::seal_singleton(
            self.scratch(),
            TypeSymbol::declared(name, self.symbols).unwrap(),
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
    values: &[Option<Holding<'cell>>],
) -> Vec<Node<'cell>> {
    let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
    let next = crate::symbols::BinderSymbol::declared("next", symbols).unwrap();
    let value = crate::symbols::BinderSymbol::declared("value", symbols).unwrap();
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
                types.record(scratch, &[(next, identity), (value, cell.concrete_ktype())]),
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
