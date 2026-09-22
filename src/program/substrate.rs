//! The substrate: an owner that borrows nothing, and the running state that borrows it.

use self_cell::self_cell;

use crate::memory::{Bump, ProgramBrand, ProgramStorage, SlabHandle, program_storage, resident};
use crate::parse::parse_with_path;
use crate::scheduler::{DrainStalled, Graph, NativeStep, Placement, Resting, Scheduler, Work};
use crate::scope::BodyShape;
use crate::symbols::SymbolInterner;
use crate::type_lattice::TypeRegistry;

use super::body;
use super::bundle::{KBirth, KBundle};
use super::record::{Language, LoadError, Program};

/// What the substrate owns outright. It borrows nothing, and `self_cell` boxes it and only ever
/// lends it shared, so everything that borrows it lives in [`Running`].
///
/// `registry` is the bump the [type lattice](crate::type_lattice)'s registry is built over — the
/// one tier that still needs a collections arena rather than program storage's write surface. It
/// is released whole with the rest, and the registry's destructor never runs.
struct Owner {
    storage: ProgramStorage,
    registry: Bump,
    symbols: SymbolInterner,
}

/// Everything that names `'graph`: the graph by value, since reclaiming a region needs exclusive
/// access, and its root, the region every root work is born under and the top level's bindings
/// live in; the registry and the program record in program storage, so what a step reads is a
/// `'graph` borrow; and what the top level left at rest when it last ran.
pub struct Running<'graph> {
    graph: Graph<'graph, KBundle>,
    root: SlabHandle,
    brand: ProgramBrand<'graph>,
    symbols: &'graph SymbolInterner,
    types: &'graph TypeRegistry<'graph>,
    program: &'graph Program<'graph>,
    resting: Option<Resting<'graph, KBundle>>,
}

impl<'graph> Running<'graph> {
    /// A drain over this substrate's graph, for the length of one call.
    pub fn scheduler(&mut self) -> Scheduler<'_, 'graph, KBundle> {
        Scheduler::over(&mut self.graph)
    }

    /// Run the program: the body runner born as a tenant of the root, so the top level's bindings
    /// are written in the root's region. What it leaves at rest is kept for [`inspect`](Self::inspect);
    /// a second run runs the program again and replaces it.
    pub fn run(&mut self) -> Result<(), DrainStalled> {
        let work = Work {
            step: body::run,
            state: KBirth::Program {
                program: self.program,
            },
        };
        self.resting = Scheduler::over(&mut self.graph).run(work, self.root, Placement::Shares)?;
        Ok(())
    }

    /// Run `step` as a later root work, a tenant of the root born from what the top level left at
    /// rest: [`KBirth::Inspect`], the view of its activation. It is how a REPL or a test reads a
    /// top-level binding after the drain; what the step leaves at rest is kept in turn. Refused as
    /// [`DrainStalled::Unfinished`] before the program has run.
    pub fn inspect(&mut self, step: NativeStep<'graph, KBundle>) -> Result<(), DrainStalled> {
        let resting = self.resting.ok_or(DrainStalled::Unfinished)?;
        self.resting =
            Scheduler::over(&mut self.graph).resume(step, resting, self.root, Placement::Shares)?;
        Ok(())
    }

    /// The program record, in program storage.
    pub fn program(&self) -> &'graph Program<'graph> {
        self.program
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
    /// Parse `source` into fresh program storage and stand the program up beside it, over a slab of
    /// `cap` cells, one of which is its root: `language`'s builtin table at `'graph`, the program's
    /// shape over it, the root, and the [`Program`] record. `cap` is at least one.
    pub fn load<L: Language>(source: &str, path: &str, cap: u32) -> Result<Self, LoadError> {
        let owner = Owner {
            storage: program_storage(),
            registry: Bump::new(),
            symbols: SymbolInterner::new(),
        };
        Joined::try_new(owner, |owner| {
            let brand = owner.storage.brand();
            let parsed =
                parse_with_path(brand, &owner.symbols, source, path).map_err(LoadError::Parse)?;
            // The registry's destructor never runs: everything it owns is bumped into the owner's
            // registry arena, which releases it whole.
            let types = &*owner
                .registry
                .alloc(TypeRegistry::in_region(&owner.registry));
            let scratch = Bump::new();
            let builtins = L::builtins(brand.writer(), &owner.symbols, types, &scratch);
            let shape = BodyShape::of_program(brand, &parsed, builtins, &scratch)
                .map_err(LoadError::Shape)?;
            let program = resident(
                brand.writer(),
                Program::new(shape, builtins, types, &owner.symbols, L::evaluator()),
            );
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
                program,
                resting: None,
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
