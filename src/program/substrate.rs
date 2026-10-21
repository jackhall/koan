//! The substrate: an owner that borrows nothing, and the running state that borrows it — and,
//! for the tests that resume a load, the [`Loaded`] door its passes' resumption goes through.

use std::cell::Cell;

use self_cell::self_cell;

use crate::elaborate::{builtin_error, resume_channel, type_channel};
use crate::knot::KBuiltins;
use crate::memory::{
    Bump, BumpAllocator, BumpVec, ProgramBrand, ProgramStorage, SlabHandle, Writer,
    program_storage, resident,
};
use crate::parse::parse_with_path;
use crate::scheduler::{
    DrainStalled, Graph, NativeStep, Placement, Resting, Roots, Scheduler, Work,
};
use crate::scope::{Binding, BodyShape, Relay, Resumption, Settled, ShapeError, Unknown};
use crate::symbols::SymbolInterner;
use crate::type_lattice::{Agent, Halves, KType, RootBuilder};
use crate::values::{Chain, Half, SpecBounds, TypeTiers, Types};

use super::body;
use super::bundle::{KBirth, KBundle};
use super::record::{Language, LoadError, Outcome, Output, Program};

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
/// access, and its two roots — the build root the load ran in, and the run root every root work is
/// born under and the top level's bindings live in; the program record in program storage, holding the type tiers, so what a step
/// reads is a `'graph` borrow; and what the top level left at rest when it last ran.
pub struct Running<'graph> {
    graph: Graph<'graph, KBundle>,
    roots: Roots,
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
    /// a second run runs the program again and replaces it. A run whose top level received an
    /// error value wrote its message to the error sink, and is [`Outcome::Uncaught`].
    pub fn run(&mut self) -> Result<Outcome, DrainStalled> {
        let work = Work {
            step: body::run,
            state: KBirth::Program {
                program: self.program,
            },
        };
        let uncaught = self.program.uncaught();
        uncaught.set(false);
        self.resting =
            Scheduler::over(&mut self.graph).run(work, self.roots.run, Placement::Shares)?;
        Ok(if uncaught.get() {
            Outcome::Uncaught
        } else {
            Outcome::Completed
        })
    }

    /// Run `step` as a later root work, a tenant of the root born from what the top level left at
    /// rest: [`KBirth::Inspect`], the view of its activation. It is how a REPL or a test reads a
    /// top-level binding after the drain; what the step leaves at rest is kept in turn. Refused as
    /// [`DrainStalled::Unfinished`] before the program has run.
    pub fn inspect(&mut self, step: NativeStep<'graph, KBundle>) -> Result<(), DrainStalled> {
        let resting = self.resting.ok_or(DrainStalled::Unfinished)?;
        self.resting = Scheduler::over(&mut self.graph).resume(
            step,
            resting,
            self.roots.run,
            Placement::Shares,
        )?;
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
        self.roots.run
    }

    /// The graph's two slab roots: the build root the load ran in, and the run root.
    pub fn roots(&self) -> Roots {
        self.roots
    }

    /// The program storage this substrate's AST and program record rest in.
    pub fn brand(&self) -> ProgramBrand<'graph> {
        self.brand
    }

    pub fn symbols(&self) -> &'graph SymbolInterner {
        self.symbols
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
    /// `cap` cells, two of which are its build root and its run root: `language`'s builtin table at
    /// `'graph`, the program's shape over it and checked by the language, and the [`Program`]
    /// record writing to `output`, all written by the build root's one step. `cap` is at least two.
    /// `bounds` sets how many live children the build root's specialization table and every
    /// specialization's own table hold.
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
        bounds: SpecBounds,
        output: Output,
    ) -> Result<Self, LoadError> {
        Self::load_with::<L, ()>(source, path, cap, bounds, output, |_| ())
            .map(|(loaded, _)| loaded)
    }

    /// [`load`](Self::load), handing `inspect` the load once its passes quiesce and before its
    /// types' root freezes: what [`Loaded::resume`] resumes. What `inspect` returns rides beside
    /// the substrate, and names nothing of the load.
    pub fn load_with<L: Language, R: 'static>(
        source: &str,
        path: &str,
        cap: u32,
        bounds: SpecBounds,
        output: Output,
        inspect: impl for<'g, 'l, 'door> FnOnce(Loaded<'g, 'l, 'door, L>) -> R,
    ) -> Result<(Self, R), LoadError> {
        let mut inspected = None;
        let owner = Owner {
            storage: program_storage(),
            registry: Bump::new(),
            agent: Bump::new(),
            symbols: SymbolInterner::new(),
        };
        Joined::try_new(owner, |owner| {
            let brand = owner.storage.brand();
            let parsed =
                parse_with_path(brand, &owner.symbols, source, path).map_err(LoadError::Parse)?;
            // The load is the build root's one step: what it writes rests in program storage and
            // the owner's registry. The load door lays the build root's header and its table, which
            // the root-level specializations fill.
            let (graph, roots, program) = Graph::load(cap, bounds, || {
                // Everything the root owns is laid in the owner's registry arena, which releases it
                // whole. The load's chain never leaves the load: its end freezes the root.
                let builder = RootBuilder::in_region(&owner.registry);
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
                let shape = shapes.root();
                type_channel(shape, builtins, types, brand.writer(), &scratch).map_err(refused)?;
                L::check(shape, builtins, types, brand.writer(), &scratch).map_err(refused)?;
                crate::build::list_seams(shape, brand.writer(), &scratch);
                crate::build::lay_keys(shape, brand.writer(), &scratch);
                inspected = Some(inspect(Loaded {
                    shape,
                    builtins,
                    types,
                    writer: brand.writer(),
                    scratch: &scratch,
                    symbols: &owner.symbols,
                    language: std::marker::PhantomData,
                }));
                let error = builtin_error(types, &owner.symbols, &scratch);
                let tiers = TypeTiers {
                    frozen: builder.freeze(),
                    agent: Agent::in_region(&owner.agent),
                };
                owner.symbols.freeze();
                let writer = brand.writer();
                let program = resident(
                    writer,
                    Program::new(
                        shape,
                        builtins,
                        tiers,
                        error,
                        &owner.symbols,
                        L::evaluator(),
                        output,
                        &writer.fill(1, |_| Cell::new(false))[0],
                    ),
                );
                Ok((tiers, program))
            })?;
            Ok(Running {
                graph,
                roots,
                brand,
                symbols: &owner.symbols,
                program,
                resting: None,
            })
        })
        .map(|joined| {
            (
                CellSubstrate(joined),
                inspected.expect("a load that succeeds is inspected"),
            )
        })
    }

    /// Reach the running state. `'graph` is fresh per call and names the same storage every time,
    /// so what one call leaves in the graph or the types' root the next call finds.
    pub fn with<R>(&mut self, f: impl for<'graph> FnOnce(&mut Running<'graph>) -> R) -> R {
        self.0.with_dependent_mut(|_, running| f(running))
    }
}

/// A load whose passes have quiesced, before its types' root freezes: its root shape, and the
/// door a resumption of it goes through. Only tests resume a load.
pub struct Loaded<'graph, 'l, 'door, L> {
    shape: &'graph BodyShape<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    types: &'l Chain<'graph, 'graph, 'graph, 'door>,
    writer: Writer<'graph, 'graph>,
    scratch: BumpAllocator<'l>,
    symbols: &'graph SymbolInterner,
    language: std::marker::PhantomData<L>,
}

impl<'graph, 'l, 'door, L: Language> Loaded<'graph, 'l, 'door, L> {
    pub fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    pub fn symbols(&self) -> &'graph SymbolInterner {
        self.symbols
    }

    /// The load's chain: what a resumption interns its types through.
    pub fn types(&self) -> &'l Chain<'graph, 'graph, 'graph, 'door> {
        self.types
    }

    /// `staged`, laid down as a binding.
    pub fn binding(&self, staged: &[(Unknown, KType)]) -> Binding<'graph> {
        let mut held = BumpVec::with_capacity_in(staged.len(), self.scratch);
        held.extend_from_slice(staged);
        Binding::laid(self.writer, &mut held)
    }

    /// Resume the load under `binding`, extending `base`, the resumptions before it, newest
    /// first: the type channel, then the language's pass. `everything` re-runs every fact, not
    /// only those the binding wakes. A binding extends its base's.
    pub fn resume(
        &self,
        base: &[&'graph Settled<'graph>],
        binding: Binding<'graph>,
        everything: bool,
    ) -> Result<&'graph Settled<'graph>, ShapeError<'graph>> {
        if let Some(newest) = base.first() {
            assert!(
                binding.extends(newest.binding),
                "a resumption's binding extends its base's"
            );
        }
        let over = Resumption {
            binding,
            base,
            everything,
        };
        let relay = Relay::new(self.writer, self.types, self.types, self.scratch);
        let channel = resume_channel(self.shape, over, self.builtins, &relay, self.scratch)?;
        L::resume(
            self.shape,
            self.builtins,
            &relay,
            self.scratch,
            over,
            channel,
        )
    }
}
