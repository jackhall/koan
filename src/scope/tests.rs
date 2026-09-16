//! Shared scaffolding for `scope`'s suites: program storage with a registry and an interner, a
//! scratch, a builtin table, and a graph to lay activations down in.

mod activation;
mod boundary;
mod examples;
pub(crate) mod plan;
mod properties;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, CellHandle, Edge, ProgramBrand, ReleaseAbsorption, SlabHandle,
    Verdict, Writer, program_storage, reattachable,
};
use crate::parse::{KExpression, LabelInterner, TypeSymbol, ValueSymbol, parse};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{Knotted, Resolved, TypeValue, Value, Weight};

use super::Builtins;

/// A continuation family for a graph whose cells only store.
struct Step;
reattachable!(Step => ());

/// A stand-in function: the index of the knot member it is, so a read through an edge capture is
/// observable without a function layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Probe(pub u32);

impl Knotted for Probe {
    fn ktype(&self) -> KType {
        KType::ANY
    }

    fn weight(&self) -> Weight {
        Weight::ZERO
    }

    fn sibling(&self, edge: Edge) -> Self {
        Probe(edge.index())
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self> {
        Resolved::Function
    }
}

/// How many cells a fixture's graph stands up — one to run in, the rest as binder handles.
const CELLS: u32 = 64;

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

    /// Every top-level line of `source`, parsed into program storage.
    pub fn parse(&self, source: &str) -> Vec<KExpression<'graph>> {
        parse(self.program, self.labels, source)
            .unwrap_or_else(|error| panic!("`{source}` parses: {error:?}"))
    }

    /// Run `step` in one cell of a graph over this fixture's storage, handing it the cell's writer
    /// and the handles of every other cell, to stand for binders.
    pub fn in_cell<R>(&self, step: impl for<'cell> FnOnce(Writer<'cell>, &[CellHandle]) -> R) -> R {
        let mut graph: CellGraph<'graph, Step> = CellGraph::new(CELLS, |_| Verdict::Pin);
        let cells: Vec<SlabHandle> = (0..CELLS)
            .map(|_| graph.create(None).expect("the graph has a free slot"))
            .collect();
        let handles: Vec<CellHandle> = cells[1..].iter().map(|cell| (*cell).into()).collect();
        let out = graph
            .enter(cells[0], |context| step(context.writer(), &handles))
            .expect("a fresh cell is enterable");
        for cell in cells {
            graph
                .release(cell, ReleaseAbsorption::IntoHolder)
                .expect("a cell outside its step releases");
        }
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

/// The value builtins every suite's table holds.
pub(super) const BUILTIN_VALUES: &[&str] = &["origin"];

/// The type builtins every suite's table holds.
pub(super) const BUILTIN_TYPES: &[&str] = &["Number", "Str", "Bool", "Null", "Any", "Ring"];

pub(super) fn value_name(text: &str, labels: &LabelInterner) -> ValueSymbol {
    ValueSymbol::declared(text, labels).expect("a value token")
}

pub(super) fn type_name(text: &str, labels: &LabelInterner) -> TypeSymbol {
    TypeSymbol::declared(text, labels).expect("a Type token")
}

/// The suites' builtin table: `origin = 0` and the scalar types, laid down in `writer`'s region.
pub(super) fn builtins<'graph, 'cell, X: Knotted>(
    fixture: &Fixture<'_, 'graph>,
    writer: Writer<'cell>,
) -> &'cell Builtins<'graph, 'cell, X> {
    let labels = fixture.labels;
    let values: Vec<_> = BUILTIN_VALUES
        .iter()
        .map(|name| (value_name(name, labels), Value::Number(0.0)))
        .collect();
    let handles = [
        KType::NUMBER,
        KType::STR,
        KType::BOOL,
        KType::NULL,
        KType::ANY,
        KType::ANY,
    ];
    let types: Vec<_> = BUILTIN_TYPES
        .iter()
        .zip(handles)
        .map(|(name, handle)| {
            let value = Value::Type(TypeValue::new(writer, handle, fixture.types));
            (type_name(name, labels), value)
        })
        .collect();
    Builtins::new(writer, fixture.scratch, &values, &types)
}
