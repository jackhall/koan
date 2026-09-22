//! Shared scaffolding for `scope`'s suites: program storage with a registry and an interner, a
//! scratch, a builtin table, and a graph to lay activations down in.

mod activation;
mod boundary;
mod examples;
mod groups;
pub(crate) mod plan;
mod properties;
mod rewrite;
mod units;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, Edge, ProgramBrand, ReleaseAbsorption, Verdict, Writer,
    covariant, program_storage, reattachable,
};
use crate::parse::{KExpression, parse};
use crate::symbols::{SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{
    DeepCopy, Knotted, KnottedFamily, Resolved, TypeValue, Value, ValueFamily, Weight,
};

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

/// The family of [`Probe`], which holds no region borrow: its form is `Probe` at every brand.
pub(super) struct ProbeFamily;

impl<'graph> KnottedFamily<'graph> for ProbeFamily {
    type Closed<'cell>
        = Probe
    where
        'graph: 'cell;

    fn copy_into<'from, 'to>(
        _: Writer<'to>,
        member: &Probe,
        _: &mut DeepCopy<'_, 'graph, 'from, 'to, Probe, Probe>,
    ) -> Probe
    where
        'graph: 'from,
        'graph: 'to,
    {
        *member
    }
}

// An activation over probes lays down a view of its slots, which is proven covariant here.
covariant!(ValueFamily<ProbeFamily>);

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

    /// Every top-level line of `source`, parsed into program storage.
    pub fn parse(&self, source: &str) -> Vec<KExpression<'graph>> {
        parse(self.program, self.symbols, source)
            .unwrap_or_else(|error| panic!("`{source}` parses: {error:?}"))
    }

    /// Run `step` in one cell of a graph over this fixture's storage, handing it the cell's writer.
    pub fn in_cell<R>(&self, step: impl for<'cell> FnOnce(Writer<'cell>) -> R) -> R {
        let mut graph: CellGraph<'graph, Step> = CellGraph::new(1, |_| Verdict::Pin);
        let cell = graph.create(None).expect("the graph has a free slot");
        let out = graph
            .enter(cell, |context| step(context.writer()))
            .expect("a fresh cell is enterable");
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
    let symbols = SymbolInterner::new();
    let scratch = Bump::new();
    test(&Fixture {
        program,
        types: &types,
        symbols: &symbols,
        scratch: &scratch,
    })
}

/// The value builtins every suite's table holds.
pub(super) const BUILTIN_VALUES: &[&str] = &["origin"];

/// The type builtins every suite's table holds.
pub(super) const BUILTIN_TYPES: &[&str] = &["Number", "Str", "Bool", "Null", "Any", "Ring"];

pub(super) fn value_name(text: &str, symbols: &SymbolInterner) -> ValueSymbol {
    ValueSymbol::declared(text, symbols).expect("a value token")
}

pub(super) fn type_name(text: &str, symbols: &SymbolInterner) -> TypeSymbol {
    TypeSymbol::declared(text, symbols).expect("a Type token")
}

/// The suites' builtin table: `origin = 0` and the scalar types, laid down in `writer`'s region.
pub(super) fn builtins<'graph, 'cell, X: Knotted>(
    fixture: &Fixture<'_, 'graph>,
    writer: Writer<'cell>,
) -> &'cell Builtins<'graph, 'cell, X> {
    let symbols = fixture.symbols;
    let values: Vec<_> = BUILTIN_VALUES
        .iter()
        .map(|name| (value_name(name, symbols), Value::Number(0.0)))
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
            (type_name(name, symbols), value)
        })
        .collect();
    Builtins::new(writer, fixture.scratch, &values, &types)
}
