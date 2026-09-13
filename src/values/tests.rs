//! Shared scaffolding for `values`' suites: program storage with a registry built in it, a scratch
//! and an interner, and a graph over that storage to run steps in.

mod boundary;
mod construction;
mod crossing;
mod equality;
mod render;
mod satisfaction;
mod working;

use crate::memory::{
    Bump, BumpAllocator, CellGraph, Prices, ProgramBrand, ReleaseAbsorption, StepContext, Verdict,
    program_storage, reattachable,
};
use crate::parse::{ExpressionPart, KExpression, LabelInterner, parse};
use crate::type_lattice::TypeRegistry;

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
