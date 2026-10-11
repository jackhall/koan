//! Shared scaffolding for `values`' suites: program storage beside the types a test runs over, a
//! scratch and an interner, a graph over that storage to run steps in, and a test-only knot member
//! whose every node is a data node, with a sealed singleton newtype to tag its rings. [`parts`] and
//! [`TestTypes`] are shared crate-wide: a suite outside `values` reads a value's parts through the
//! door as a program does, and lays its types out as a load and a run lay them.

mod boundary;
mod construction;
mod crossing;
mod depth;
mod digest;
mod equality;
mod habitat;
mod render;
mod satisfaction;
mod surface;
mod working;

use super::digest::{ContentDigest, DigestHasher, Tag};
use super::{Half, TypeHeader, Types};
use crate::memory::{
    Active, Bump, BumpAllocator, CellGraph, CellHandle, Covariant, Delivery, Edge, Header,
    KnotPlan, Member, NoDelivery, NoScratch, Operand, Prices, ProgramBrand, Reattachable,
    ReattachableOverBoth, ReleaseAbsorption, Stale, StepContext, Verdict, Writer, covariant,
    program_storage, reattachable,
};
use crate::parse::{ExpressionPart, KExpression, ProgramNode, parse};
use crate::symbols::{SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, RecursiveGroupWindow, RelativeSchema, Root, RootBuilder,
};
use crate::values::{
    Circular, CodeView, DeepCopy, Knotted, KnottedFamily, Resolved, Seen, ValueCarrier,
    ValueFamily, Weight, cross_view,
};

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
    types: &Types<'_, '_>,
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

/// The types a test runs over, laid out as a load and a run lay them: a root builder the test's
/// load-time work interns into through [`load`](Self::load), a seeded root, and a chain per habitat
/// half that reads the root, then the builder's live table as its region tier, then the half's own
/// branch — so a type a test interned outside a cell, or a shape built inside one, resolves in
/// every cell, and a type a cell's value door builds lands in that cell's branch.
pub(crate) struct TestTypes<'graph> {
    builder: &'graph RootBuilder<'graph>,
    root: &'graph Root<'graph>,
}

impl<'graph> TestTypes<'graph> {
    /// A builder and a root, both laid in `bump`.
    pub fn new(bump: &'graph Bump) -> Self {
        TestTypes {
            builder: RootBuilder::in_region(bump),
            root: Root::seeded(bump),
        }
    }

    /// The chain a test's load-time work goes through: the builder, as the load's chain.
    pub fn load(&self) -> Types<'graph, 'graph> {
        Types::building(Half::Load(self.builder))
    }

    /// The chain for the half `writer` lays into, whose header door is `header`.
    pub fn half<'here>(
        &self,
        writer: Writer<'here>,
        header: Header<'graph, 'here, TypeHeader>,
    ) -> Types<'graph, 'here> {
        Types::scratch(
            self.root,
            Half::Load(self.builder),
            Half::of(writer, header),
        )
    }

    /// The chain for what `context`'s step lays in its cell.
    pub fn of<'here, C, S, D>(
        &self,
        context: &StepContext<'graph, '_, 'here, '_, C, S, D, TypeHeader>,
    ) -> Types<'graph, 'here>
    where
        C: Reattachable<'graph>,
        S: ReattachableOverBoth<'graph>,
        D: Delivery<'graph>,
    {
        self.half(context.writer(), context.header())
    }

    /// [`values::cross`](crate::values::cross) over this test's chains.
    pub fn cross<'step, C, S, D, XF>(
        &self,
        context: &mut StepContext<'graph, 'step, '_, '_, C, S, D, TypeHeader>,
        dest: impl Into<CellHandle>,
        carrier: &ValueCarrier<'graph, 'step, XF>,
    ) -> Result<ValueCarrier<'graph, 'step, XF>, Stale<CellHandle>>
    where
        C: Reattachable<'graph>,
        S: ReattachableOverBoth<'graph>,
        D: Delivery<'graph>,
        XF: KnottedFamily<'graph>,
        ValueFamily<XF>: Covariant<'graph>,
    {
        let weight = context.read(carrier).value().weight();
        context.alloc_into::<ValueFamily<XF>, ValueFamily<XF>>(
            dest,
            &[Operand {
                carrier,
                copy_bytes: weight.bytes(),
            }],
            |writer, header, views| {
                Active::new(cross_view(writer, &self.half(writer, header), &views[0]))
            },
        )
    }

    /// [`values::cross_here`](crate::values::cross_here) over this test's chains.
    pub fn cross_here<'step, 'here, C, S, D, XF>(
        &self,
        context: &mut StepContext<'graph, 'step, 'here, '_, C, S, D, TypeHeader>,
        carrier: &ValueCarrier<'graph, 'step, XF>,
    ) -> crate::values::Value<'here, XF::Closed<'here>>
    where
        C: Reattachable<'graph>,
        S: ReattachableOverBoth<'graph>,
        D: Delivery<'graph>,
        XF: KnottedFamily<'graph>,
        ValueFamily<XF>: Covariant<'graph>,
    {
        let weight = context.read(carrier).value().weight();
        context.alloc_here(
            &[Operand {
                carrier,
                copy_bytes: weight.bytes(),
            }],
            |writer, header, views| cross_view(writer, &self.half(writer, header), &views[0]),
        )
    }
}

/// A chain that reads `root` and interns anything a test asks of it into a builder of its own in
/// `bump`: how a test reads the types a load built, outside any step.
pub(crate) fn reading<'tier>(root: &'tier Root<'tier>, bump: &'tier Bump) -> Types<'tier, 'tier> {
    Types::new(root, Half::Load(RootBuilder::in_region(bump)))
}

/// A continuation family for a graph whose cells only store.
pub(super) struct Step;
reattachable!(Step => ());

/// A graph whose cells only store, keeping a type branch per habitat half.
pub(super) type TestGraph<'graph> = CellGraph<'graph, Step, NoScratch, NoDelivery, TypeHeader>;

/// What a test reads and writes outside the graph. `'graph` is the program storage the parsed AST
/// and the types live in, which a graph built over it borrows.
pub(super) struct Fixture<'f, 'graph> {
    pub program: ProgramBrand<'graph>,
    /// The load's chain, which a test interns through outside a cell.
    pub types: &'f Types<'graph, 'graph>,
    /// The tiers every cell's chain is built over.
    pub tiers: &'f TestTypes<'graph>,
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
            &mut StepContext<
                'graph,
                'step,
                'here,
                'scratch,
                Step,
                NoScratch,
                NoDelivery,
                TypeHeader,
            >,
        ) -> R,
    ) -> R {
        let mut graph: TestGraph<'graph> = CellGraph::new(1, verdict);
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
    // The lattice's tiers keep a bump of their own; program storage is the one `cellgraph` store
    // everything else here is written into.
    let registry = Bump::new();
    let tiers = TestTypes::new(&registry);
    let types = tiers.load();
    let symbols = SymbolInterner::new();
    let scratch = Bump::new();
    test(&Fixture {
        program,
        types: &types,
        tiers: &tiers,
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

    /// A stand-in holds no value: its knot is its identity.
    fn held<'a>(&self, _: &Types<'_, '_>, _: BumpAllocator<'_>, _: &mut dyn FnMut(Seen<'a, Self>))
    where
        Self: 'a,
    {
    }

    fn digest_held(
        &self,
        _: &Types<'_, '_>,
        _: BumpAllocator<'_>,
        _: &mut dyn FnMut() -> ContentDigest,
    ) -> ContentDigest {
        DigestHasher::new(Tag::Knot)
            .feed(self.identity())
            .finished()
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

    /// Every value link of every node of the knot, nodes in index order, as the knot layer lists
    /// a data node's.
    fn held<'a>(
        &self,
        types: &Types<'_, '_>,
        scratch: BumpAllocator<'_>,
        out: &mut dyn FnMut(Seen<'a, Self>),
    ) where
        Self: 'a,
    {
        for node in self.0.knot().members() {
            Circular::content_parts(Node(node), types, scratch, out);
        }
    }

    /// The knot's node count and each node's content in index order, as the knot layer hashes a
    /// knot.
    fn digest_held(
        &self,
        types: &Types<'_, '_>,
        scratch: BumpAllocator<'_>,
        parts: &mut dyn FnMut() -> ContentDigest,
    ) -> ContentDigest {
        let knot = self.0.knot();
        let mut hasher = DigestHasher::new(Tag::Knot);
        hasher.count(knot.len() as usize);
        for node in knot.members() {
            hasher.digest(Circular::content(Node(node), types, scratch, parts));
        }
        hasher.finished()
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

    fn carried_types<'from>(member: &Node<'from>, out: &mut dyn FnMut(DeclaredType<Parametric>))
    where
        'graph: 'from,
    {
        for node in member.0.knot().members() {
            out(node.payload().ktype().into());
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
