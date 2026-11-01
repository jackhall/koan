//! The substrate: an owner that borrows nothing, and the running state that borrows it. A run whose
//! top level a kept fault reached renders it once the drain ends, walking the links of the cells it
//! kept, and answers an [`Uncaught`] that holds those cells until the host drops it.

use std::cell::Cell;

use self_cell::self_cell;

use crate::builtin_image;
use crate::knot::KBuiltins;
use crate::memory::{
    Bump, BumpAllocator, ProgramBrand, ProgramStorage, SlabHandle, program_storage, resident,
};
use crate::parse::parse_with_path;
use crate::scheduler::{DrainStalled, Graph, NativeStep, Placement, Resting, Scheduler, Work};
use crate::scope::{Inner, Link, Trace};
use crate::symbols::SymbolInterner;
use crate::type_lattice::{Agent, Halves, RootBuilder};
use crate::values::{BuildBudget, Chain, Half, KeptCell, TypeTiers, Types};

use super::body;
use super::bundle::{KBirth, KBundle};
use super::record::{Ending, Language, LoadError, Output, Program};

/// The stack a program's load and run need: the walks over parsed syntax recurse once per nested
/// part, so a program nested [`MAX_SYNTAX_DEPTH`](crate::parse::MAX_SYNTAX_DEPTH) deep fits in it
/// in a debug build. A host runs a program on a thread of this size rather than on whatever stack
/// its platform's main thread has.
pub const STACK_BYTES: usize = 64 << 20;

/// What the substrate owns outright. It borrows nothing, and `self_cell` boxes it and only ever
/// lends it shared, so everything that borrows it lives in [`Running`].
///
/// `registry` is the bump the [type lattice](crate::type_lattice)'s load is built over, and
/// `agent` the bump its agent tier grows in: the tiers that still need a collections arena rather
/// than program storage's write surface. Both are released whole with the rest, and nothing laid
/// in them runs a destructor.
struct Owner {
    storage: ProgramStorage,
    registry: Bump,
    agent: Bump,
    symbols: SymbolInterner,
}

/// Everything that names `'graph`: the graph by value, since reclaiming a region needs exclusive
/// access, and its run root, which every root work is born under and the top level's bindings
/// live in; the program record in program storage, holding the type tiers, so what a step reads
/// is a `'graph` borrow; and what the top level left at rest when it last ran.
pub struct Running<'graph> {
    graph: Graph<'graph, KBundle>,
    root: SlabHandle,
    brand: ProgramBrand<'graph>,
    symbols: &'graph SymbolInterner,
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
    /// a second run runs the program again and replaces it. A run whose top level a fault reached
    /// wrote its diagnostic to the error sink, and is [`Outcome::Uncaught`], holding the cells the
    /// fault kept until it is dropped.
    pub fn run(&mut self) -> Result<Outcome<'_, 'graph>, DrainStalled> {
        let work = Work {
            step: body::run,
            state: KBirth::Program {
                program: self.program,
            },
        };
        let ending = self.program.ending();
        ending.set(Ending::Completed);
        self.resting = Scheduler::over(&mut self.graph).run(work, self.root, Placement::Shares)?;
        let outermost = match ending.get() {
            Ending::Completed => return Ok(Outcome::Completed),
            Ending::Rendered => None,
            Ending::Kept(outermost) => {
                let scratch = Bump::new();
                (self.program.output().error)(self.traced(outermost, &scratch).rendered());
                Some(outermost)
            }
        };
        Ok(Outcome::Uncaught(Uncaught {
            running: self,
            outermost,
        }))
    }

    /// The diagnostic of the uncaught fault whose outermost kept cell is `outermost`, gathered in
    /// `scratch` by walking its links from cell to cell, each read through its own region's chain.
    fn traced<'x>(&self, outermost: KeptCell, scratch: &'x Bump) -> Trace<'x> {
        let mut trace = Trace::new(scratch);
        let mut next = Some(outermost);
        while let Some(cell) = next.take() {
            next = (self.graph)
                .read_kept(cell, |link, types| {
                    trace.gathered(Inner::Link(link), self.symbols, types)
                })
                .flatten();
        }
        trace
    }

    /// Give the run root back, and answer whether the graph is then empty: what a test reads to see
    /// a run leave no cell behind. The substrate runs nothing after.
    #[cfg(test)]
    pub(crate) fn emptied(&mut self) -> bool {
        self.graph.release_root(self.root).is_ok() && self.graph.is_empty()
    }

    /// [`run`](Self::run), ended: how the run ended, with any cell an uncaught fault kept released.
    #[cfg(test)]
    pub(crate) fn ran(&mut self) -> Result<Ran, DrainStalled> {
        Ok(match self.run()? {
            Outcome::Completed => Ran::Completed,
            Outcome::Uncaught(_) => Ran::Uncaught,
        })
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

    /// The graph's run root: a storage-only slab cell taken at load, which no drain enters or
    /// releases, so every root work is born under it and a result built there outlives the drain
    /// that built it.
    pub fn root(&self) -> SlabHandle {
        self.root
    }

    /// The program storage this substrate's AST and program record rest in.
    pub fn brand(&self) -> ProgramBrand<'graph> {
        self.brand
    }

    pub fn symbols(&self) -> &'graph SymbolInterner {
        self.symbols
    }
}

/// How a program's run ended: completed, or with an uncaught fault.
pub enum Outcome<'a, 'graph> {
    Completed,
    Uncaught(Uncaught<'a, 'graph>),
}

impl Outcome<'_, '_> {
    /// Whether a fault reached the top level.
    pub fn is_uncaught(&self) -> bool {
        matches!(self, Outcome::Uncaught(_))
    }
}

/// A run a fault ended uncaught: it borrows the running state, so nothing reads it until this is
/// dropped, and holds the cells the fault kept — the outermost, whose link leads to the rest —
/// which its drop releases. A fault the top level raised itself kept none.
pub struct Uncaught<'a, 'graph> {
    running: &'a mut Running<'graph>,
    outermost: Option<KeptCell>,
}

impl Uncaught<'_, '_> {
    /// How many cells the fault kept that are still live: every one, until this is dropped.
    #[cfg(test)]
    pub(crate) fn kept_live(&self) -> usize {
        let (mut live, mut next) = (0, self.outermost);
        while let Some(kept) = next {
            next = match self.running.graph.read_kept(kept, |link, _| below(link)) {
                Some(below) => {
                    live += 1;
                    below
                }
                None => None,
            };
        }
        live
    }
}

impl Drop for Uncaught<'_, '_> {
    /// Release every cell the fault kept, each read for the next before it goes; kept cells
    /// release in any order.
    fn drop(&mut self) {
        let graph = &mut self.running.graph;
        let mut next = self.outermost.take();
        while let Some(kept) = next {
            next = (graph.read_kept(kept, |link, _| below(link))).flatten();
            let released = graph.release_kept(kept);
            debug_assert!(
                released.is_ok(),
                "a kept cell is live until its outcome drops"
            );
        }
    }
}

/// The kept cell below `link`'s region, if its fault passed through one.
fn below<V>(link: &Link<'_, V>) -> Option<KeptCell> {
    let mut link = link;
    loop {
        match link.inner {
            Inner::Link(inner) => link = inner,
            Inner::Below { kept, .. } => return Some(kept),
            Inner::Fault(_) => return None,
        }
    }
}

/// How a run ended, with nothing held: what a test compares.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ran {
    Completed,
    Uncaught,
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

/// The load's one walk over `shapes`: every record the language's walk computes, laid in program
/// storage through `writer`, beside the folds the load lays over them.
pub(crate) fn walked<'graph, L: Language>(
    shapes: &'graph crate::scope::Shapes<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    types: &Chain<'graph, 'graph, 'graph, '_>,
    writer: crate::memory::Writer<'graph, 'graph>,
    scratch: BumpAllocator<'_>,
) -> Result<&'graph crate::scope::LoadFacts<'graph>, crate::scope::ShapeError<'graph>> {
    let store = crate::build::LoadStore::new(writer, scratch, shapes, crate::scope::Binding::EMPTY);
    L::facts(
        shapes,
        builtins,
        &store,
        types,
        scratch,
        shapes.root().segment(),
    )?;
    let unfolded = resident(writer, store.facts(scratch));
    let folds = crate::build::fold(unfolded, writer, scratch);
    Ok(resident(writer, unfolded.folded(writer, folds)))
}

impl CellSubstrate {
    /// Parse `source` into fresh program storage and stand the program up beside it, over a slab of
    /// `cap` cells, one of which is its run root: `language`'s builtin table at `'graph`, the
    /// program's shape over it and checked by the language, and the [`Program`] record writing to
    /// `output`, all written by the load before the graph stands. `cap` is at least one. `budget`
    /// bounds the bytes of the settlements no run cell holds.
    ///
    /// The load builds its types through one chain over a root builder, and freezes it when the
    /// load ends, so no step can intern into it: a step's types rest in the agent tier laid beside
    /// it, and a name recorded after the load in the symbol table's overlay. A refused shape is rendered into its
    /// [`LoadError`] on the way out, while the interner and the chain its names and types render
    /// through still stand; a load that succeeds renders nothing.
    pub fn load<L: Language>(
        source: &str,
        path: &str,
        cap: u32,
        budget: BuildBudget,
        output: Output,
    ) -> Result<Self, LoadError> {
        let owner = Owner {
            storage: program_storage(),
            registry: Bump::new(),
            agent: Bump::new(),
            symbols: SymbolInterner::resting_on(&builtin_image::SYMBOLS),
        };
        Joined::try_new(owner, |owner| {
            let brand = owner.storage.brand();
            let parsed =
                parse_with_path(brand, &owner.symbols, source, path).map_err(LoadError::Parse)?;
            // What the load writes rests in program storage and the owner's registry.
            let (graph, root, program) = Graph::load(cap, budget, || {
                // Everything the root owns is laid in the owner's registry arena, which releases it
                // whole. The load's chain never leaves the load: its end freezes the root.
                let builder = RootBuilder::over(&owner.registry, &builtin_image::IMAGE);
                let types: &Chain<'_, '_, '_, '_> =
                    &Types::building(Halves::alone(Half::load(builder)));
                let scratch = Bump::new();
                let builtins = L::builtins(brand.writer(), &owner.symbols, types, &scratch);
                let refused = |error: crate::scope::ShapeError<'_>| LoadError::Shape {
                    rendered: error.display(&owner.symbols, types).to_string(),
                };
                let shapes = crate::scope::Shapes::of_program(
                    brand,
                    &parsed,
                    builtins,
                    types,
                    &owner.symbols,
                    &scratch,
                )
                .map_err(refused)?;
                let walked = walked::<L>(shapes, builtins, types, brand.writer(), &scratch)
                    .map_err(refused)?;
                let tiers = TypeTiers {
                    frozen: builder.freeze(),
                    agent: Agent::in_region(&owner.agent),
                };
                owner.symbols.freeze();
                let writer = brand.writer();
                let program = resident(
                    writer,
                    Program::new(
                        walked,
                        builtins,
                        tiers,
                        (
                            builtin_image::RESULT,
                            builtin_image::ERROR,
                            builtin_image::ERROR_VARIANTS,
                        ),
                        &owner.symbols,
                        (
                            L::evaluator(),
                            super::settle::settled::<L>,
                            super::settle::composed::<L>,
                            super::activation_store::activated_in::<L>,
                        ),
                        output,
                        &writer.fill(1, |_| Cell::new(Ending::Completed))[0],
                    ),
                );
                Ok((tiers, (walked, program), program))
            })?;
            Ok(Running {
                graph,
                root,
                brand,
                symbols: &owner.symbols,
                program,
                resting: None,
            })
        })
        .map(CellSubstrate)
    }

    /// Reach the running state. `'graph` is fresh per call and names the same storage every time,
    /// so what one call leaves in the graph or the types' root the next call finds.
    pub fn with<R>(&mut self, f: impl for<'graph> FnOnce(&mut Running<'graph>) -> R) -> R {
        self.0.with_dependent_mut(|_, running| f(running))
    }

    /// The bytes laid in program storage so far: what a test reads to see a run lay nothing
    /// there.
    #[cfg(test)]
    pub(crate) fn storage_bytes(&self) -> usize {
        self.0.borrow_owner().storage.allocated_bytes()
    }
}
