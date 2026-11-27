//! The substrate: an owner that borrows nothing, and the running state that borrows it. A run whose
//! top level a kept fault reached renders it once the drain ends, walking the links of the cells it
//! kept, and answers an [`Uncaught`] that holds those cells until the host drops it.

use std::cell::Cell;

use self_cell::self_cell;

use crate::build::lay::{BindingGraph, GraphCell, GraphFacts, Instances, Layout};
use crate::builtin_image;
use crate::knot::KBuiltins;
use crate::memory::{
    Bump, BumpAllocator, ProgramBrand, ProgramStorage, SlabHandle, program_storage, resident,
};
use crate::parse::parse_with_path;
use crate::scheduler::{DrainStalled, Graph, NativeStep, Placement, Resting, Scheduler, Work};
use crate::scope::{Address, Inner, Link, LoadFacts, NameAddress, ShapeError, Trace};
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
    /// What the binding graph's instances rest in: those the load made, frozen.
    binding: Bump,
    /// The program's binding graph, set once by the load.
    graph: GraphCell,
    /// What the instances the run makes rest in, and their views.
    instances: Bump,
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
    instances: &'graph Instances<'graph>,
}

impl<'graph> Running<'graph> {
    /// The program's binding graph, beside the instances the load made of it.
    pub fn binding_graph(&self) -> &BindingGraph<'graph> {
        &self.instances.binding
    }

    /// The instances the run makes of the program's binding graph.
    #[cfg(test)]
    pub(crate) fn instances(&self) -> &'graph Instances<'graph> {
        self.instances
    }

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

/// The binding graph of the base `load` holds, its facts laid through `writer`, and the instances
/// the load makes of it from the root, frozen in `lend`: the program's, or a composition's. A recursion whose calls build a name
/// over itself refuses the load, naming a call on it; so does a refusal an instance the load
/// makes answers, under the first call in text order into its body.
pub(crate) fn bound_graph<'graph, L: Language>(
    load: &'graph LoadFacts<'graph>,
    types: &Chain<'graph, 'graph, 'graph, '_>,
    writer: crate::memory::Writer<'graph, 'graph>,
    scratch: BumpAllocator<'_>,
    symbols: &SymbolInterner,
    (lend, place): (&'graph Bump, &'graph GraphCell),
) -> Result<BindingGraph<'graph>, ShapeError<'graph>> {
    let crate::build::lay::Folded { text, facts } =
        crate::build::lay::folded(load, types, scratch, writer);
    // A question left out has no owner a run can read its answer through.
    debug_assert_eq!(
        facts.left, 0,
        "the fold leaves no stored question to the run"
    );
    let facts = resident(writer, facts);
    let graph = match text.lay(facts) {
        Ok(graph) => match place.set(graph) {
            Ok(()) => place.get().expect("the graph was set"),
            Err(_) => panic!("a base's graph is laid once"),
        },
        Err(bindgraph::LayError::Grows { edge, name }) => {
            return Err(grows(load, facts, edge, name));
        }
        Err(bindgraph::LayError::Malformed(malformed)) => {
            panic!("the load's records fold to a malformed text: {malformed}")
        }
    };
    let empty = resident(lend, bindgraph::Frozen::empty());
    let resting = bindgraph::Resting::new(lend, None);
    let mut bound = bindgraph::Bound::new(graph, facts, empty, lend, None);
    if let Some((question, refusal, entered)) =
        L::instances(&mut bound, resting, facts, load, types, scratch, symbols)
    {
        let refusal = crate::scope::relaid_error(writer, refusal);
        // The call that entered the refusing instance; the root's own, or a holder's an onward
        // call entered, names the first call into the question's body, where there is one.
        let call = match entered {
            Some(edge) if graph.edge(edge).onward().is_none() => facts.call(edge),
            _ => facts.first_call(graph.question(question).site().body()),
        };
        return Err(match call {
            Some(call) => ShapeError::UnderCall { call, refusal },
            None => *refusal,
        });
    }
    #[cfg(feature = "perf")]
    let counts = bound.counts();
    let frozen = bound.freeze(resting);
    Ok(BindingGraph {
        graph,
        facts,
        frozen,
        layout: Layout::of(graph, writer, scratch),
        #[cfg(feature = "perf")]
        counts,
    })
}

/// The refusal of a recursion whose call `edge` builds `name` over itself.
fn grows<'graph>(
    load: &LoadFacts<'graph>,
    facts: &GraphFacts<'graph>,
    edge: bindgraph::Edge,
    name: bindgraph::NameId,
) -> ShapeError<'graph> {
    let shapes = load.shapes();
    let NameAddress::Site(unknown) = facts.name(name).address else {
        panic!("a name a call passes is a site name");
    };
    let shape = shapes.shape_at(unknown.segment);
    let grown = match unknown.at {
        Address::Slot(slot) => shape.slot_name(slot),
        Address::Capture(capture) => shape.captures()[capture.index()].name,
        Address::Held(_) => {
            unreachable!("a call passes entry names, and a part held in place binds at a statement")
        }
    };
    let bindgraph::Edge::Passes(edge) = edge else {
        panic!("the fold lays no answering edge");
    };
    let written = || shapes.written(unknown.segment).map(|form| form.source);
    let at = (facts.call(edge)).or_else(written);
    ShapeError::Grows {
        name: grown,
        at: at.expect("a callable body is written"),
    }
}

/// The load's one walk over `shapes`: every record the language's walk computes, laid in program
/// storage through `writer`, beside the folds the load lays over them.
pub(crate) fn walked<'graph, 'x, L: Language>(
    shapes: &'graph crate::scope::Shapes<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    types: &Chain<'graph, 'graph, 'graph, '_>,
    writer: crate::memory::Writer<'graph, 'graph>,
    scratch: BumpAllocator<'x>,
    symbols: &'x crate::symbols::SymbolInterner,
) -> Result<&'graph crate::scope::LoadFacts<'graph>, crate::scope::ShapeError<'graph>> {
    let spellings = crate::scope::Spellings::new(symbols);
    let store = crate::build::LoadStore::new(writer, scratch, (shapes, spellings));
    L::facts(shapes, builtins, &store, types, scratch)?;
    let unfolded = resident(writer, store.facts(scratch));
    let folds = crate::build::fold(unfolded, writer, scratch);
    let folded = resident(writer, unfolded.folded(writer, folds));
    Ok(folded)
}

impl CellSubstrate {
    /// Parse `source` into fresh program storage and stand the program up beside it, over a slab of
    /// `cap` cells, one of which is its run root: `language`'s builtin table at `'graph`, the
    /// program's shape over it and checked by the language, and the [`Program`] record writing to
    /// `output`, all written by the load before the graph stands. `cap` is at least one. `budget`
    /// bounds the bytes of the compositions no run cell holds.
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
            binding: Bump::new(),
            graph: GraphCell::new(),
            instances: Bump::new(),
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
                let walked = walked::<L>(
                    shapes,
                    builtins,
                    types,
                    brand.writer(),
                    &scratch,
                    &owner.symbols,
                )
                .map_err(refused)?;
                let binding = bound_graph::<L>(
                    walked,
                    types,
                    brand.writer(),
                    &scratch,
                    &owner.symbols,
                    (&owner.binding, &owner.graph),
                )
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
                        (L::evaluator(), super::compose::composed::<L>),
                        output,
                        &writer.fill(1, |_| Cell::new(Ending::Completed))[0],
                    ),
                );
                Ok((tiers, (walked, program), (program, binding)))
            })?;
            let (program, binding) = program;
            let instances = resident(
                &owner.instances,
                Instances::new(
                    binding,
                    (
                        program.facts(),
                        crate::scope::Spellings::new(&owner.symbols),
                    ),
                    (L::rebind, L::instance),
                    &owner.instances,
                    budget.bytes.get(),
                ),
            );
            let mut graph = graph;
            graph.lend_instances(instances);
            Ok(Running {
                graph,
                root,
                brand,
                symbols: &owner.symbols,
                program,
                resting: None,
                instances,
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
