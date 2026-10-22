//! The test stores a resumption runs the walk through: a [`Layer`] over a load's facts, binding
//! more than its base, and a [`Frame`], whose binding is read off the values a frame's slots hold.
//! Each re-runs only the units its binding wakes, and reads every other record from its base. They
//! live in `program` because it is the one module that names both the build's store vocabulary and
//! the values a frame binds ([the store](../../../roadmap/gradual-build/one-walk-build.md)).
//!
//! The **test door** resumes a loaded program after its types' root freezes, through
//! [`Running::resume`], [`Running::reload`] and [`Running::in_frame`]: each walks the program again
//! under its store, interning what the binding closes through the chain it is handed — one over the
//! frozen root, so successive resumptions share what they intern — and laying its records in the
//! bump it is handed. It folds what it wrote, so a test reads a resumption's facts through the same
//! accessors the run uses.

use crate::build::LoadStore;
use crate::memory::{Bump, BumpAllocator, BumpVec, Lays, Writer, resident};
use crate::program::{CellSubstrate, KBirth, KBundle, KState, Language, Running};
use crate::scheduler::{Action, Step, StepError};
use crate::scope::tests::with_shapes;
use crate::scope::{
    Address, Binding, Bound, Closing, Folds, Frontier, Keyed, LoadFacts, Segment, ShapeError,
    Shapes, Site, Slot, Store, Tree, Typed, UnitFacts, UnitId, Unknown, Writes, each_part,
};
use crate::type_lattice::{KType, display_name};
use crate::values::{Chain, Knotted, Nothing, TypeValue, Value};

use super::{WANTED, wanted};

/// A resumption's store over `base`: its binding extends the base's, and a unit re-runs where its
/// frontier meets an unknown it binds that the base's does not, or everywhere under `everything`.
pub struct Layer<'o, 'p, L> {
    lay: L,
    base: &'o LoadFacts<'o>,
    binding: Binding<'o>,
    /// What `binding` binds that the base's does not: what wakes a record.
    fresh: Binding<'o>,
    everything: bool,
    writes: Writes<'o, 'p>,
}

impl<'o, 'p, L: Lays<'o>> Layer<'o, 'p, L> {
    /// A layer over `base` binding `binding`, which binds every unknown the base's binds alike.
    pub fn new(
        lay: L,
        scratch: BumpAllocator<'p>,
        base: &'o LoadFacts<'o>,
        binding: Binding<'o>,
        everything: bool,
    ) -> Self {
        assert!(
            binding.extends(base.binding()),
            "a layer binds what its base binds"
        );
        let mut fresh = BumpVec::new_in(scratch);
        fresh.extend(
            (binding.entries().iter())
                .filter(|(unknown, _)| base.binding().bound(*unknown).is_none())
                .copied(),
        );
        Layer {
            lay,
            base,
            binding,
            fresh: Binding::laid(lay, &mut fresh),
            everything,
            writes: Writes::new(base.shapes(), scratch),
        }
    }

    /// Every record this layer wrote, over its base's, beside `folds` by segment: its base's where
    /// it is empty.
    pub fn facts(&self, scratch: BumpAllocator<'_>, folds: &[Folds<'o>]) -> LoadFacts<'o> {
        (self.writes).laid(self.lay, scratch, Some(self.base), self.binding, folds)
    }
}

impl<'o, L: Lays<'o>> Store<'o> for Layer<'o, '_, L> {
    type Lay = L;

    fn lay(&self) -> L {
        self.lay
    }

    fn binding(&self, unknown: Unknown) -> Option<Bound<'o>> {
        self.binding.bound(unknown)
    }

    fn fact(&self, unit: UnitId) -> Option<(&'o UnitFacts<'o>, Frontier<'o>)> {
        self.writes.unit(unit).or_else(|| self.base.unit(unit))
    }

    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)> {
        (self.writes.closing(body)).or_else(|| self.base.closing(body))
    }

    fn wakes(&self, frontier: Frontier<'_>) -> bool {
        self.everything || frontier.meets_binding(self.fresh)
    }

    fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        self.writes.write(unit, facts, frontier);
    }

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        self.writes.write_close(body, closing, frontier);
    }

    fn refuse(&self, error: ShapeError<'o>) -> ShapeError<'o> {
        error
    }
}

/// A resumption's store inside a frame: a [`Layer`] over the load's facts whose binding is what
/// the frame's slots hold — a type value binds its type, and a module value its carried type over
/// its content, by digest.
pub struct Frame<'o, 'p, L>(Layer<'o, 'p, L>);

/// What a frame's slot holding `value` binds its unknown to: a type value its type, and a module
/// value its carried type over its content, by digest.
pub fn bound_of<'o, X: Knotted>(
    lay: impl Lays<'o>,
    value: Value<'_, X>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Bound<'o> {
    if let Some(named) = value.as_type() {
        return Bound::Type(named.handle());
    }
    assert!(
        value.as_module().is_some(),
        "a frame binds a type or a module"
    );
    Bound::Module {
        carried: value.concrete_ktype(),
        tree: resident(lay, Tree::Digest(value.digest(types, scratch))),
    }
}

impl<'o, 'p, L: Lays<'o>> Frame<'o, 'p, L> {
    /// A frame over `load`, the load's own facts, whose slots hold `slots`.
    pub fn new<X: Knotted>(
        lay: L,
        scratch: BumpAllocator<'p>,
        load: &'o LoadFacts<'o>,
        slots: &[(Unknown, Value<'_, X>)],
        types: &Chain<'_, '_, '_, '_>,
    ) -> Self {
        let mut bound = BumpVec::new_in(scratch);
        bound.extend(
            (slots.iter())
                .map(|(unknown, value)| (*unknown, bound_of(lay, *value, types, scratch))),
        );
        let binding = Binding::laid(lay, &mut bound);
        Frame(Layer::new(lay, scratch, load, binding, false))
    }

    /// What the frame's slots bind.
    pub fn binding(&self) -> Binding<'o> {
        self.0.binding
    }
}

impl<'o, L: Lays<'o>> Store<'o> for Frame<'o, '_, L> {
    type Lay = L;

    fn lay(&self) -> L {
        self.0.lay()
    }

    fn binding(&self, unknown: Unknown) -> Option<Bound<'o>> {
        self.0.binding(unknown)
    }

    fn fact(&self, unit: UnitId) -> Option<(&'o UnitFacts<'o>, Frontier<'o>)> {
        self.0.fact(unit)
    }

    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)> {
        self.0.close(body)
    }

    fn wakes(&self, frontier: Frontier<'_>) -> bool {
        self.0.wakes(frontier)
    }

    fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>) {
        self.0.write(unit, facts, frontier);
    }

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>) {
        self.0.write_close(body, closing, frontier);
    }

    fn refuse(&self, error: ShapeError<'o>) -> ShapeError<'o> {
        self.0.refuse(error)
    }
}

impl<'graph> Running<'graph> {
    /// Resume the load under `binding`, extending `base` — a resumption over the load, or the load's
    /// own facts where `None`: the units it wakes re-run, or every unit under `everything`.
    pub fn resume<'l, L: Language>(
        &self,
        types: &Chain<'l, '_, '_, '_>,
        base: Option<&'l LoadFacts<'l>>,
        binding: Binding<'l>,
        everything: bool,
        bump: &'l Bump,
    ) -> Result<&'l LoadFacts<'l>, ShapeError<'l>>
    where
        'graph: 'l,
    {
        let scratch = Bump::new();
        let base = base.unwrap_or(self.program().facts());
        let layer = Layer::new(bump, &scratch, base, binding, everything);
        self.walked::<L, _>(&layer, types, &scratch)?;
        folded(layer.facts(&scratch, &[]), bump, &scratch)
    }

    /// Walk the whole program again under `binding`, through a [`LoadStore`].
    pub fn reload<'l, L: Language>(
        &self,
        types: &Chain<'l, '_, '_, '_>,
        binding: Binding<'l>,
        bump: &'l Bump,
    ) -> Result<&'l LoadFacts<'l>, ShapeError<'l>>
    where
        'graph: 'l,
    {
        let scratch = Bump::new();
        let store = LoadStore::new(bump, &scratch, self.program().shapes(), binding);
        self.walked::<L, _>(&store, types, &scratch)?;
        folded(store.facts(&scratch, &[]), bump, &scratch)
    }

    /// Resume the load in a [`Frame`] whose slots hold `slots`.
    pub fn in_frame<'l, L: Language, X: Knotted>(
        &self,
        types: &Chain<'l, '_, '_, '_>,
        slots: &[(Unknown, Value<'_, X>)],
        bump: &'l Bump,
    ) -> Result<&'l LoadFacts<'l>, ShapeError<'l>>
    where
        'graph: 'l,
    {
        let scratch = Bump::new();
        let load: &'l LoadFacts<'l> = self.program().facts();
        let frame = Frame::new(bump, &scratch, load, slots, types);
        self.walked::<L, _>(&frame, types, &scratch)?;
        folded(frame.0.facts(&scratch, &[]), bump, &scratch)
    }

    /// Run `L`'s walk over the program into `store`, interning through `types`.
    fn walked<'l, L: Language, S: Store<'l>>(
        &self,
        store: &S,
        types: &Chain<'l, '_, '_, '_>,
        scratch: &Bump,
    ) -> Result<(), ShapeError<'l>>
    where
        'graph: 'l,
    {
        let program = self.program();
        L::facts(program.shapes(), program.builtins(), store, types, scratch)
    }
}

/// `unfolded` laid in `bump` beside its folds, as the load lays its own.
fn folded<'l>(
    unfolded: LoadFacts<'l>,
    bump: &'l Bump,
    scratch: &Bump,
) -> Result<&'l LoadFacts<'l>, ShapeError<'l>> {
    let unfolded = resident(bump, unfolded);
    let folds = crate::build::fold(unfolded, bump, scratch)?;
    Ok(resident(bump, unfolded.folded(bump, folds)))
}

/// What a [`Frame`] binds the top-level binding `name` to, read off the resting top level once the
/// program has run: `type <name>`, or `module <carried type> <digest>`.
pub(crate) fn frame_bound_back(substrate: &mut CellSubstrate, name: &str) -> String {
    super::evaluator::reset();
    WANTED.with(|wanted| *wanted.borrow_mut() = vec![name.to_string()]);
    substrate.with(|running| {
        running
            .inspect(framing)
            .expect("the inspection runs to completion")
    });
    super::evaluator::recorded().concat()
}

/// [`frame_bound_back`]'s root work: it records what a frame binds the wanted binding to.
fn framing<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, view }) = state else {
        return step.failed(StepError::Refused);
    };
    let name = WANTED.with(|wanted| wanted.borrow()[0].clone());
    let scratch = Bump::new();
    let types = &step.types();
    let named = |ktype| display_name(ktype, types, program.symbols()).to_string();
    let rendered = match bound_of(&scratch, wanted(program, &view, &name), types, &scratch) {
        Bound::Type(handle) => format!("type {}", named(handle)),
        Bound::Module { carried, tree } => {
            let digest = tree
                .digest()
                .expect("a frame binds a module to a closed tree");
            format!("module {} {digest:?}", named(carried))
        }
    };
    super::evaluator::record(rendered);
    step.leave(birth)
}

/// What renders a frame's facts for [`framed`]: the facts, the step's chain they render through,
/// the program's names, and the names a render leaves out.
pub(crate) type Framer = fn(
    &LoadFacts<'_>,
    &Chain<'_, '_, '_, '_>,
    &crate::symbols::SymbolInterner,
    &[String],
) -> Vec<String>;

/// What [`framed`] hands its step, and what the step hands back.
struct Framing {
    slots: Vec<(Unknown, String)>,
    render: Framer,
    leave: Vec<String>,
    rendered: Option<Result<Vec<String>, String>>,
}

thread_local! {
    static FRAMING: std::cell::RefCell<Option<Framing>> = const { std::cell::RefCell::new(None) };
}

/// Resume the load of a program that has run in a [`Frame`] whose slots hold what the top-level
/// bindings `slots` name, each beside the unknown it binds, inside a step at its own brand, and
/// render what it settles through `render`, `leave` left out: or its refusal.
pub(crate) fn framed<L: Language>(
    running: &mut Running<'_>,
    slots: Vec<(Unknown, String)>,
    render: Framer,
    leave: Vec<String>,
) -> Result<Vec<String>, String> {
    FRAMING.with(|held| {
        *held.borrow_mut() = Some(Framing {
            slots,
            render,
            leave,
            rendered: None,
        })
    });
    running
        .inspect(framing_facts::<L>)
        .expect("the inspection runs to completion");
    (FRAMING.with(|held| held.borrow_mut().take()))
        .and_then(|framing| framing.rendered)
        .expect("the step framed the load")
}

/// [`framed`]'s root work.
fn framing_facts<'graph, L: Language>(
    step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, view }) = state else {
        return step.failed(StepError::Refused);
    };
    let (slots, render, leave) = FRAMING.with(|held| {
        let held = held.borrow();
        let framing = held.as_ref().expect("framed hands its slots");
        (framing.slots.clone(), framing.render, framing.leave.clone())
    });
    let (bump, scratch) = (Bump::new(), Bump::new());
    let types = &step.types();
    let values: Vec<_> = (slots.iter())
        .map(|(unknown, name)| (*unknown, wanted(program, &view, name)))
        .collect();
    let load: &LoadFacts<'_> = program.facts();
    let frame = Frame::new(&bump, &scratch, load, &values, types);
    let walked = L::facts(
        program.shapes(),
        program.builtins(),
        &frame,
        types,
        &scratch,
    )
    .and_then(|()| folded(frame.0.facts(&scratch, &[]), &bump, &scratch));
    let rendered = match walked {
        Ok(facts) => Ok(render(facts, types, program.symbols(), &leave)),
        Err(error) => Err(error.display(program.symbols(), types).to_string()),
    };
    FRAMING.with(|held| {
        if let Some(framing) = held.borrow_mut().as_mut() {
            framing.rendered = Some(rendered);
        }
    });
    step.leave(birth)
}

/// Three top-level statements, each its own unit.
const SOURCE: &str = "LET a = 1\nLET b = (a + 2)\nPRINT b";

/// Each unit of `shapes`'s root that runs a statement, by statement.
fn units(shapes: &Shapes<'_>) -> Vec<UnitId> {
    let root = shapes.root();
    (0..root.body().len())
        .map(|index| UnitId {
            segment: root.segment(),
            unit: root.unit_of_statement(index),
        })
        .collect()
}

/// A record over statement `index` of `shapes`'s root that settles every site of it, or mints at
/// every site of it: so a read tells which record answered.
fn record<'graph>(
    writer: Writer<'graph, 'graph>,
    shapes: &'graph Shapes<'graph>,
    index: usize,
    settles: bool,
) -> &'graph UnitFacts<'graph> {
    let root = shapes.root();
    let mut sites = Vec::new();
    each_part(&root.body()[index], |part| sites.push(Site::of(part)));
    let sites = Keyed::sorted(writer, &mut sites);
    let typed = match settles {
        true => Typed {
            settled: sites,
            ..Typed::EMPTY
        },
        false => Typed {
            minting: sites,
            ..Typed::EMPTY
        },
    };
    resident(
        writer,
        UnitFacts {
            typed: Keyed::sorted(writer, &mut [(root.segment(), typed)]),
            ..UnitFacts::EMPTY
        },
    )
}

/// Every site of statement `index` of `shapes`'s root.
fn sites_of(shapes: &Shapes<'_>, index: usize) -> Vec<Site> {
    let mut sites = Vec::new();
    each_part(&shapes.root().body()[index], |part| {
        sites.push(Site::of(part))
    });
    sites
}

/// The unknown at slot `slot` of the root.
fn unknown(shapes: &Shapes<'_>, slot: u32) -> Unknown {
    Unknown {
        segment: shapes.root().segment(),
        at: Address::Slot(Slot(slot)),
    }
}

#[test]
fn a_layer_reads_its_base_where_it_did_not_write() {
    with_shapes(SOURCE, |shapes, writer, scratch, _| {
        let units = units(shapes);
        let load = LoadStore::new(writer, scratch, shapes, Binding::EMPTY);
        for (index, unit) in units.iter().enumerate() {
            load.write(*unit, record(writer, shapes, index, true), Frontier::EMPTY);
        }
        let base = resident(writer, load.facts(scratch, &[]));
        let layer = Layer::new(writer, scratch, base, Binding::EMPTY, false);
        let rerun = record(writer, shapes, 1, false);
        layer.write(units[1], rerun, Frontier::EMPTY);
        let read = |unit| layer.fact(unit).expect("a record").0;
        assert!(std::ptr::eq(read(units[0]), base.unit(units[0]).unwrap().0));
        assert!(std::ptr::eq(read(units[1]), rerun));
        let laid = layer.facts(scratch, &[]);
        let root = shapes.root();
        for index in 0..units.len() {
            for site in sites_of(shapes, index) {
                // The re-run record replaces the base's whole: its unit's sites settle no more.
                assert_eq!(laid.settled(root, site), index != 1, "statement {index}");
                assert_eq!(laid.mints_at(root, site), index == 1, "statement {index}");
            }
        }
    });
}

#[test]
fn a_layer_wakes_exactly_where_a_frontier_meets_an_unknown_its_base_left_unbound() {
    with_shapes(SOURCE, |shapes, writer, scratch, _| {
        let [a, b, c] = [0, 1, 2].map(|slot| unknown(shapes, slot));
        let binding = |bound: &[(Unknown, KType)]| {
            let mut staged = BumpVec::new_in(scratch);
            staged.extend(
                bound
                    .iter()
                    .map(|(unknown, ktype)| (*unknown, Bound::Type(*ktype))),
            );
            Binding::laid(writer, &mut staged)
        };
        let load = LoadStore::new(writer, scratch, shapes, binding(&[(a, KType::NUMBER)]));
        let base = resident(writer, load.facts(scratch, &[]));
        let both = binding(&[(a, KType::NUMBER), (b, KType::STR)]);
        let frontier = |unknowns: &[Unknown]| {
            Frontier::from_sorted(writer.fill(unknowns.len(), |at| unknowns[at]))
        };
        let cases: [(&[Unknown], bool); 6] = [
            (&[], false),
            (&[a], false),
            (&[b], true),
            (&[c], false),
            (&[a, c], false),
            (&[a, b], true),
        ];
        let layer = Layer::new(writer, scratch, base, both, false);
        let everything = Layer::new(writer, scratch, base, both, true);
        for (unknowns, wakes) in cases {
            assert_eq!(layer.wakes(frontier(unknowns)), wakes, "{unknowns:?}");
            assert!(everything.wakes(frontier(unknowns)), "{unknowns:?}");
        }
        assert!(layer.binding(b).is_some() && layer.binding(c).is_none());
    });
}

#[test]
fn a_frame_binds_what_its_slots_hold_and_wakes_where_that_meets_a_frontier() {
    with_shapes(SOURCE, |shapes, writer, scratch, types| {
        let [a, b] = [0, 1].map(|slot| unknown(shapes, slot));
        let load = LoadStore::new(writer, scratch, shapes, Binding::EMPTY);
        let base = resident(writer, load.facts(scratch, &[]));
        let named = TypeValue::new(writer, KType::NUMBER, types);
        let slots: [(Unknown, Value<'_, Nothing>); 1] = [(a, Value::Type(named))];
        let frame = Frame::new(writer, scratch, base, &slots, types);
        assert!(
            matches!(frame.binding().bound(a), Some(Bound::Type(bound)) if bound == KType::NUMBER)
        );
        assert!(frame.binding().bound(b).is_none());
        let frontier = |unknown| Frontier::from_sorted(writer.fill(1, |_| unknown));
        assert!(frame.wakes(frontier(a)) && !frame.wakes(frontier(b)));
    });
}
