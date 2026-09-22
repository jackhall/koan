//! The substrate: an owner that borrows nothing, and the running state that borrows it.

use self_cell::self_cell;

use crate::memory::{ProgramBrand, ProgramStorage, SlabHandle, program_storage};
use crate::parse::{KExpression, ParseError, parse_with_path};
use crate::program::Steps;
use crate::scheduler::{Graph, Scheduler};
use crate::symbols::SymbolInterner;
use crate::type_lattice::TypeRegistry;

/// What the substrate owns outright. It borrows nothing, and `self_cell` boxes it and only ever
/// lends it shared, so everything that borrows it lives in [`Running`].
struct Owner {
    storage: ProgramStorage,
    symbols: SymbolInterner,
}

/// Everything that names `'graph`: the graph by value, since reclaiming a region needs exclusive
/// access, and its root, the region every root work is born under; and the registry and the parsed
/// program in program storage, so a record laid down there can borrow them at `'graph`.
pub struct Running<'graph> {
    graph: Graph<'graph, Steps>,
    root: SlabHandle,
    brand: ProgramBrand<'graph>,
    symbols: &'graph SymbolInterner,
    types: &'graph TypeRegistry<'graph>,
    statements: &'graph [KExpression<'graph>],
}

impl<'graph> Running<'graph> {
    /// A drain over this substrate's graph, for the length of one call.
    pub fn scheduler(&mut self) -> Scheduler<'_, 'graph, Steps> {
        Scheduler::over(&mut self.graph)
    }

    /// The graph's root: a storage-only slab cell taken at load, which no drain enters or releases,
    /// so every root work is born under it and a result built there outlives the drain that built
    /// it.
    pub fn root(&self) -> SlabHandle {
        self.root
    }

    /// The program storage this substrate's AST and registry rest in.
    pub fn brand(&self) -> ProgramBrand<'graph> {
        self.brand
    }

    pub fn symbols(&self) -> &'graph SymbolInterner {
        self.symbols
    }

    pub fn types(&self) -> &'graph TypeRegistry<'graph> {
        self.types
    }

    /// The parsed program's top-level statements, in source order.
    pub fn statements(&self) -> &'graph [KExpression<'graph>] {
        self.statements
    }
}

self_cell!(
    struct Joined {
        owner: Owner,
        #[not_covariant]
        dependent: Running,
    }
);

/// A parsed program and the cell graph that runs it, as one value that can be returned, moved and
/// kept. `'graph` is invariant, so the running state is reached only through [`with`](Self::with).
pub struct CellSubstrate(Joined);

impl CellSubstrate {
    /// Parse `source` into fresh program storage and stand the graph up beside it, over a slab of
    /// `cap` cells, one of which is its root. `cap` is at least one.
    pub fn load(source: &str, path: &str, cap: u32) -> Result<Self, ParseError> {
        let owner = Owner {
            storage: program_storage(),
            symbols: SymbolInterner::new(),
        };
        Joined::try_new(owner, |owner| {
            let brand = owner.storage.brand();
            let parsed = parse_with_path(brand, &owner.symbols, source, path)?;
            let statements = brand.allocator().alloc_slice_copy(&parsed);
            // The registry's destructor never runs: everything it owns is bumped into program
            // storage, which releases it whole.
            let types = &*brand
                .allocator()
                .alloc(TypeRegistry::in_region(brand.allocator()));
            let mut graph = Graph::new(cap);
            let root = graph
                .root()
                .expect("a fresh slab of at least one cell admits its root");
            Ok(Running {
                graph,
                root,
                brand,
                symbols: &owner.symbols,
                types,
                statements,
            })
        })
        .map(CellSubstrate)
    }

    /// Reach the running state. `'graph` is fresh per call and names the same storage every time,
    /// so what one call leaves in the graph or the registry the next call finds.
    pub fn with<R>(&mut self, f: impl for<'graph> FnOnce(&mut Running<'graph>) -> R) -> R {
        self.0.with_dependent_mut(|_, running| f(running))
    }
}
