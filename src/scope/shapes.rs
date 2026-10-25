//! The **shape table**: every shape one load builds, by [`Segment`], beside where each sits in the
//! body enclosing it. It is laid once, where the root's shape is built, and every fact of the text
//! that spans bodies — where an unknown binds, the levels a chain numbers its type names at, the
//! layout a closure carries — is read through it, so no shape needs a pointer to any other. A step
//! reads it through [`Code`], built per step at the step's own lifetime, so what a cell parks names a
//! segment or a site rather than a shape.
//!
//! See [README.md § Load-time types](README.md#load-time-types).

use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpVec, ProgramBrand, Writer, bump_table, collect,
};
use crate::parse::{KExpression, Role};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::values::{Chain, Knotted};

use super::builtins::Builtins;
use super::facts::{Address, UnitId, Unknown};
use super::groups::Claims;
use super::shape::{
    BodyShape, CaptureChain, CaptureSlot, CaptureSource, ComponentIndex, Coordinate, Lands, PathId,
    Position, Segment, ShapeError, ShapeKind, Site, Slot, Target, UnitIndex, follow,
};
use super::signature::declare_quantifiers;
use super::typed::{Handoff, ViewSlots};

mod memos;
mod tails;

use memos::Memos;
use tails::Tails;

/// What a step reads shapes through, at the step's own lifetime `'step`: the shape table of the base
/// its cell's chain rests on. Built per step from the cell's arena reference; what crosses a park
/// holds a segment or a site instead, so keeping one past the step does not compile:
///
/// ```compile_fail
/// use koan::memory::{Writer, resident};
/// use koan::scope::Code;
///
/// fn keep<'here>(writer: Writer<'here, '_>, code: Code<'_>) -> &'here Code<'here> {
///     resident(writer, code)
/// }
/// ```
#[derive(Clone, Copy)]
pub struct Code<'step>(&'step Shapes<'step>);

impl<'step> Code<'step> {
    /// The code `shapes` holds.
    pub fn of(shapes: &'step Shapes<'step>) -> Self {
        Code(shapes)
    }

    /// The shape at `segment`.
    pub fn shape(&self, segment: Segment) -> &'step BodyShape<'step> {
        self.0.shape_at(segment)
    }

    /// The shape table itself.
    pub fn shapes(&self) -> &'step Shapes<'step> {
        self.0
    }
}

/// Every shape one load built, by segment, and each one's place in its encloser.
#[derive(Clone, Copy)]
pub struct Shapes<'graph> {
    /// Each shape, at its segment. The root seals last, so it is the last.
    bodies: &'graph [&'graph BodyShape<'graph>],
    /// Each shape's encloser and the statement of it the shape sits in, at its segment: `None` for
    /// the root.
    parents: &'graph [Option<(Segment, u32)>],
    /// Each shape's levels, at its segment.
    levels: &'graph [Levels<'graph>],
    /// Each shape's tail-call facts, at its segment.
    tails: &'graph [Tails<'graph>],
    /// Each shape's digest memo facts, at its segment.
    memos: &'graph [Memos<'graph>],
}

/// The levels one shape numbers along its chain.
#[derive(Clone, Copy)]
struct Levels<'graph> {
    /// Each run-bound type name the shape declares, by level, beside where its activation holds
    /// it: its own `FOR ALL` names in written order, its other type names in slot order, then — at
    /// a chain root — its captures naming a type.
    homes: &'graph [(usize, Target)],
    /// [`homes`](Self::homes) by where the activation holds each name, sorted.
    levels_by_home: &'graph [(Target, usize)],
    /// The level of each of [`BodyShape::spelled`]'s paths, parallel to it: `None` for one that
    /// names no identity.
    paths: &'graph [Option<usize>],
    /// At a chain root: the level past every name level of the chain, and how many spelled
    /// identities its paths take from there.
    root: Option<(usize, usize)>,
    /// A callable's or a `MODULE` body's closure layout past its builder captures: each run-bound
    /// type name of the bodies around it on its chain that is bound when its birth runs, by level,
    /// beside where the birth reads it in the activation around it.
    layout: &'graph [(usize, Coordinate)],
}

impl Levels<'_> {
    /// Where the activation holds the name at `level`, where the shape declares it: its homes'
    /// levels run on from the first.
    fn home(&self, level: usize) -> Option<Target> {
        let (base, _) = self.homes.first()?;
        let (_, home) = self.homes.get(level.checked_sub(*base)?)?;
        Some(*home)
    }
}

impl<'graph> Shapes<'graph> {
    /// The table of a load that built no shape.
    pub const EMPTY: Self = Shapes {
        bodies: &[],
        parents: &[],
        levels: &[],
        tails: &[],
        memos: &[],
    };

    /// The shapes of a program's top-level statements and of every body nested in them.
    pub fn of_program<X: Knotted>(
        brand: ProgramBrand<'graph>,
        statements: &[KExpression<'graph>],
        builtins: &Builtins<'_, X>,
        types: &Chain<'graph, 'graph, 'graph, '_>,
        symbols: &SymbolInterner,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'graph Shapes<'graph>, ShapeError<'graph>> {
        super::shape::build_program(brand, statements, builtins, types, symbols, scratch)
    }

    /// The shapes of code composed at run time, built alone under the program's `claims`: see
    /// [`build::code`](super::shape::build_code).
    pub(crate) fn of_code<X: Knotted>(
        brand: ProgramBrand<'graph>,
        syntax: &KExpression<'graph>,
        claims: &'graph Claims<'graph>,
        builtins: &Builtins<'_, X>,
        types: &Chain<'graph, 'graph, 'graph, '_>,
        symbols: &SymbolInterner,
        scratch: BumpAllocator<'_>,
    ) -> &'graph Shapes<'graph> {
        super::shape::build_code(brand, syntax, claims, builtins, types, symbols, scratch)
    }

    /// The table over `bodies` and `parents`, its levels numbered along each chain.
    pub(super) fn laid(
        writer: Writer<'graph, '_>,
        scratch: BumpAllocator<'_>,
        bodies: &'graph [&'graph BodyShape<'graph>],
        parents: &'graph [Option<(Segment, u32)>],
    ) -> Self {
        debug_assert_eq!(bodies.len(), parents.len());
        debug_assert!(
            parents.last().is_some_and(Option::is_none),
            "the root seals last, and has no encloser"
        );
        let mut levels = BumpVec::with_capacity_in(bodies.len(), scratch);
        levels.resize(
            bodies.len(),
            Levels {
                homes: &[],
                levels_by_home: &[],
                paths: &[],
                root: None,
                layout: &[],
            },
        );
        for body in bodies {
            if matches!(body.kind(), ShapeKind::Program | ShapeKind::Code) {
                number_chain(writer, scratch, body, &mut levels);
            }
        }
        // A body's encloser seals after it, so walking the segments down lays each encloser's
        // layout before any body nested in it reads it.
        for body in bodies.iter().rev() {
            if matches!(body.kind(), ShapeKind::Callable | ShapeKind::Module) {
                let table = Table {
                    bodies,
                    parents,
                    levels: &levels,
                };
                let layout = table.layout_of(writer, scratch, body.segment());
                levels[body.segment().index() as usize].layout = layout;
            }
        }
        let mut shapes = Shapes {
            bodies,
            parents,
            levels: collect(writer, levels.iter().copied()),
            tails: &[],
            memos: &[],
        };
        let tails = tails::tails(writer, scratch, &shapes);
        shapes.tails = collect(writer, tails.iter().copied());
        let memos = memos::memos(writer, scratch, &shapes);
        shapes.memos = collect(writer, memos.iter().copied());
        shapes
    }

    /// The program's own shape.
    pub fn root(&self) -> &'graph BodyShape<'graph> {
        self.bodies.last().expect("a load builds its root")
    }

    /// The shape at `segment`.
    pub fn shape_at(&self, segment: Segment) -> &'graph BodyShape<'graph> {
        self.bodies[segment.index() as usize]
    }

    /// Every shape, by segment.
    pub fn bodies(&self) -> &'graph [&'graph BodyShape<'graph>] {
        self.bodies
    }

    /// The body enclosing `segment`, and the statement of it `segment` sits in: `None` for the root.
    pub fn parent(&self, segment: Segment) -> Option<(Segment, u32)> {
        self.parents[segment.index() as usize]
    }

    /// The nearest body at or around `segment` that owns units: the program, a callable or a
    /// quote's code. A block and a `MODULE` or `GROUP` body run inline in their encloser's unit.
    pub fn owner(&self, mut segment: Segment) -> Segment {
        loop {
            match self.shape_at(segment).kind() {
                ShapeKind::Program | ShapeKind::Callable | ShapeKind::Code => return segment,
                ShapeKind::Block | ShapeKind::Module => {
                    segment = self
                        .parent(segment)
                        .expect("an inline body has an encloser")
                        .0;
                }
            }
        }
    }

    /// The unit of `segment` that runs its statement `statement`. An inline body's units keep
    /// records of their own, as an owning body's do.
    pub fn running(&self, segment: Segment, statement: u32) -> UnitId {
        UnitId {
            segment,
            unit: self.shape_at(segment).unit_of_statement(statement as usize),
        }
    }

    /// The unit that births `body`, a callable or a quote's code, or runs it, an inline body:
    /// `None` for the root.
    pub fn birth_unit(&self, body: Segment) -> Option<UnitId> {
        let (parent, statement) = self.parent(body)?;
        Some(self.running(parent, statement))
    }

    /// The unit whose record holds the facts of the slot `slot` of `segment`: the unit binding it.
    pub fn slot_unit(&self, segment: Segment, slot: Slot) -> UnitId {
        UnitId {
            segment,
            unit: self.shape_at(segment).unit_of_slot(slot),
        }
    }

    /// The body `hops` enclosers out from `segment`: `None` past the root.
    pub fn up(&self, mut segment: Segment, hops: u32) -> Option<Segment> {
        for _ in 0..hops {
            segment = self.parent(segment)?.0;
        }
        Some(segment)
    }

    /// Whether `outer` is `inner` or a body around it. A body's segment exceeds every body's it
    /// encloses ([`Segment`]), so the walk up stops once it reaches `outer`'s segment.
    pub fn encloses(&self, outer: Segment, mut inner: Segment) -> bool {
        while inner < outer {
            match self.parent(inner) {
                Some((parent, _)) => inner = parent,
                None => return false,
            }
        }
        inner == outer
    }

    /// How many bodies enclose `segment`: `0` for the root.
    pub fn depth(&self, mut segment: Segment) -> usize {
        let mut depth = 0;
        while let Some((parent, _)) = self.parent(segment) {
            segment = parent;
            depth += 1;
        }
        depth
    }
}

/// Where an unknown binds: its body's depth on the chain, the body, and the unit there. Two sites on
/// one chain order as the binding does — an outer body before an inner one, and in one body its entry
/// before its units in order — so a frontier's seam is the latest of its members' sites.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BindSite {
    pub depth: u32,
    pub segment: Segment,
    pub unit: UnitIndex,
}

impl Shapes<'_> {
    /// Where `unknown` binds. A callable's parameter, `FOR ALL` name or `:Type` parameter, a
    /// block's parameter — a `USING … SCOPE` block's surfaced name, an arm's `it` — and a quote's
    /// code's capture bind at that body's entry; a local or a type binder at the unit that binds
    /// it.
    pub fn bind_site(&self, unknown: Unknown) -> BindSite {
        let segment = unknown.segment;
        let declaring = self.shape_at(segment);
        let at = |segment, unit| BindSite {
            depth: self.depth(segment) as u32,
            segment,
            unit,
        };
        let slot = match unknown.at {
            Address::Capture(_) => return at(segment, UnitIndex::ENTRY),
            Address::Slot(slot) => slot,
        };
        match declaring.position_of(slot) == Position::PARAMETER {
            true => at(segment, UnitIndex::ENTRY),
            false => at(segment, declaring.unit_of_slot(slot)),
        }
    }
}

impl Shapes<'_> {
    /// Each run-bound type name `segment` declares, by level, beside where its activation holds it.
    pub fn homes(&self, segment: Segment) -> &[(usize, Target)] {
        self.levels[segment.index() as usize].homes
    }

    /// Where `segment`'s activation holds the run-bound type name at `level`, where it declares it.
    pub fn home(&self, segment: Segment, level: usize) -> Option<Target> {
        self.levels[segment.index() as usize].home(level)
    }

    /// The level of the run-bound type name `address` of `segment`; `None` for any other address.
    pub fn level(&self, segment: Segment, address: Address) -> Option<usize> {
        let target = match address {
            Address::Slot(slot) => Target::Local(slot),
            Address::Capture(capture) => Target::Capture(capture),
        };
        let by_home = self.levels[segment.index() as usize].levels_by_home;
        let at = by_home
            .binary_search_by_key(&target, |(home, _)| *home)
            .ok()?;
        Some(by_home[at].1)
    }

    /// The level of the path `segment`'s types spell at index `index` of [`BodyShape::spelled`];
    /// `None` where it names no identity.
    pub fn path_level(&self, segment: Segment, index: usize) -> Option<usize> {
        self.levels[segment.index() as usize].paths[index]
    }

    /// The chain root `segment` lies on: the program, or the quote's code around it.
    pub fn chain_root(&self, mut segment: Segment) -> Segment {
        loop {
            match self.shape_at(segment).kind() {
                ShapeKind::Program | ShapeKind::Code => return segment,
                _ => segment = self.parent(segment).expect("a chain has a root").0,
            }
        }
    }

    /// The level past every name and spelled path level of the chain `root` roots: where the paths
    /// no type spells take theirs.
    pub fn path_base(&self, root: Segment) -> usize {
        let (base, spelled) = self.levels[root.index() as usize]
            .root
            .expect("a chain root numbers its paths");
        base + spelled
    }

    /// The first path level of the chain `root` roots, past every name level of it.
    pub fn spelled_base(&self, root: Segment) -> usize {
        (self.levels[root.index() as usize].root)
            .expect("a chain root numbers its paths")
            .0
    }
}

/// Number the chain `root` roots: each shape's names after its encloser's, siblings alike, then
/// each spelled path identity past every name level, in first-spelling order over the chain's
/// shapes in program order.
fn number_chain<'graph>(
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'_>,
    root: &'graph BodyShape<'graph>,
    levels: &mut BumpVec<'_, Levels<'graph>>,
) {
    let mut order = BumpVec::new_in(scratch);
    let top = number_names(writer, scratch, root, true, 0, levels, &mut order);
    // Each path identity's index, handed out in first-spelling order.
    let mut identities: BumpBackedMap<'_, PathId<'graph>, usize> = bump_table(scratch);
    for shape in order.iter() {
        let mut paths = BumpVec::with_capacity_in(shape.spelled().len(), scratch);
        for spelled in shape.spelled() {
            paths.push(spelled.id.map(|id| {
                let next = identities.len();
                top + *identities.entry(id).or_insert(next)
            }));
        }
        levels[shape.segment().index() as usize].paths = collect(writer, paths.iter().copied());
    }
    levels[root.segment().index() as usize].root = Some((top, identities.len()));
}

/// Number the names `shape` declares from `base`, then each shape nested in it on its chain from
/// past them; `order` gathers the chain's shapes in program order. Returns the first level past
/// every name level of the subtree.
fn number_names<'graph>(
    writer: Writer<'graph, '_>,
    scratch: BumpAllocator<'_>,
    shape: &'graph BodyShape<'graph>,
    root: bool,
    base: usize,
    levels: &mut BumpVec<'_, Levels<'graph>>,
    order: &mut BumpVec<'_, &'graph BodyShape<'graph>>,
) -> usize {
    order.push(shape);
    let mut homes = BumpVec::new_in(scratch);
    let mut own = BumpVec::new_in(scratch);
    if let Some(form) = shape.form() {
        own_quantifiers(form, &mut own);
    }
    let slot_of = |name| shape.slot(name).map(|(slot, _)| slot);
    for name in own.iter() {
        if let Some(slot) = slot_of(*name) {
            homes.push(Target::Local(slot));
        }
    }
    for index in 0..shape.slots() {
        let slot = Slot(index as u32);
        let name = shape.slot_name(slot);
        if matches!(name, BinderSymbol::Type(_)) && !own.contains(&name) {
            homes.push(Target::Local(slot));
        }
    }
    if root {
        for (index, capture) in shape.captures().iter().enumerate() {
            if matches!(capture.name, BinderSymbol::Type(_)) {
                homes.push(Target::Capture(CaptureSlot(index as u32)));
            }
        }
    }
    let next = base + homes.len();
    let laid = writer.fill(homes.len(), |index| (base + index, homes[index]));
    let mut by_home = BumpVec::with_capacity_in(homes.len(), scratch);
    by_home.extend(laid.iter().map(|(level, home)| (*home, *level)));
    by_home.sort_unstable();
    let shape_levels = &mut levels[shape.segment().index() as usize];
    shape_levels.homes = laid;
    shape_levels.levels_by_home = collect(writer, by_home.iter().copied());
    let mut deepest = next;
    for (_, nested) in shape.nested_shapes() {
        if nested.kind() != ShapeKind::Code {
            let past = number_names(writer, scratch, nested, false, next, levels, order);
            deepest = deepest.max(past);
        }
    }
    deepest
}

/// The `FOR ALL` names a callable's `form` writes, in written order.
fn own_quantifiers(form: &KExpression<'_>, into: &mut BumpVec<'_, BinderSymbol>) {
    let Some(shape) = form.cache().builtin_shape() else {
        return;
    };
    for (role, part) in shape.roles().zip(form.parts) {
        if role == Role::Quantifiers {
            declare_quantifiers(&part.value, into);
        }
    }
}

impl Shapes<'_> {
    /// The closure layout of `segment` past its builder captures, a callable's or a `MODULE`
    /// body's: entry `i` is closure slot `captures().len() + i`. Empty for any other kind.
    pub fn layout(&self, segment: Segment) -> &[(usize, Coordinate)] {
        self.levels[segment.index() as usize].layout
    }

    /// Where an activation of `segment` reads the run-bound type name at `level`: its own slot or
    /// capture, a hop per block around it, or a slot of its closure's layout. `None` past the chain
    /// root, and for a level no body around it binds when it runs.
    pub fn coordinate_of(&self, segment: Segment, level: usize) -> Option<Coordinate> {
        let table = Table {
            bodies: self.bodies,
            parents: self.parents,
            levels: self.levels,
        };
        table.coordinate_of(segment, level)
    }
}

/// The shape table's parts, over levels laid or still being laid.
struct Table<'a, 'graph> {
    bodies: &'a [&'graph BodyShape<'graph>],
    parents: &'a [Option<(Segment, u32)>],
    levels: &'a [Levels<'graph>],
}

impl<'graph> Table<'_, 'graph> {
    fn shape_at(&self, segment: Segment) -> &'graph BodyShape<'graph> {
        self.bodies[segment.index() as usize]
    }

    fn parent(&self, segment: Segment) -> Option<(Segment, u32)> {
        self.parents[segment.index() as usize]
    }

    /// The layout of `segment`, a callable or a `MODULE` body: see [`Levels::layout`]. Reads the
    /// layouts of the bodies around it, which must be laid.
    fn layout_of(
        &self,
        writer: Writer<'graph, '_>,
        scratch: BumpAllocator<'_>,
        segment: Segment,
    ) -> &'graph [(usize, Coordinate)] {
        let mut entries = BumpVec::new_in(scratch);
        let (mut holder, mut statement) = self.parent(segment).expect("a nested body has a parent");
        let reader = holder;
        loop {
            let enclosing = self.shape_at(holder);
            let birth = enclosing.unit_of_statement(statement as usize);
            for (level, home) in self.levels[holder.index() as usize].homes {
                let bound = match home {
                    Target::Local(slot) => enclosing.unit_of_slot(*slot) < birth,
                    Target::Capture(_) => true,
                };
                if bound {
                    let read = self
                        .coordinate_of(reader, *level)
                        .expect("a name bound around a birth reads there");
                    entries.push((*level, read));
                }
            }
            if matches!(enclosing.kind(), ShapeKind::Program | ShapeKind::Code) {
                break;
            }
            (holder, statement) = self.parent(holder).expect("a nested body has a parent");
        }
        entries.sort_unstable_by_key(|(level, _)| *level);
        collect(writer, entries.iter().copied())
    }

    fn coordinate_of(&self, segment: Segment, level: usize) -> Option<Coordinate> {
        let mut at = segment;
        let mut hops = 0;
        loop {
            let levels = &self.levels[at.index() as usize];
            if let Some(home) = levels.home(level) {
                return Some(Coordinate::Activation { hops, target: home });
            }
            let shape = self.shape_at(at);
            match shape.kind() {
                ShapeKind::Block => {
                    at = self.parent(at)?.0;
                    hops += 1;
                }
                ShapeKind::Callable | ShapeKind::Module => {
                    let index = (levels.layout)
                        .binary_search_by_key(&level, |(held, _)| *held)
                        .ok()?;
                    return Some(Coordinate::Activation {
                        hops,
                        target: Target::Capture(CaptureSlot(
                            (shape.captures().len() + index) as u32,
                        )),
                    });
                }
                ShapeKind::Program | ShapeKind::Code => return None,
            }
        }
    }
}

impl CaptureChain for Shapes<'_> {
    type At = Segment;

    fn outward(&self, segment: Segment, hops: u32) -> Option<Segment> {
        self.up(segment, hops)
    }

    fn capture_source(&self, segment: Segment, capture: CaptureSlot) -> CaptureSource {
        Shapes::capture_source(self, segment, capture)
    }

    fn member(&self, segment: Segment, component: ComponentIndex, index: u32) -> Slot {
        self.shape_at(segment).components()[component.index()].members[index as usize]
    }
}

impl Shapes<'_> {
    /// Where `coordinate`, read in `segment`, lands once each capture is followed to its source;
    /// `None` past the chain.
    pub fn follow(&self, segment: Segment, coordinate: Coordinate) -> Option<Lands> {
        follow(self, segment, coordinate, |_, _| ())
    }

    /// The source of capture `capture` of `segment`: a builder capture's own, or a type capture
    /// of its layout, which reads its coordinate outside.
    pub fn capture_source(&self, segment: Segment, capture: CaptureSlot) -> CaptureSource {
        let builder = self.shape_at(segment).captures();
        match builder.get(capture.index()) {
            Some(spec) => spec.source,
            None => CaptureSource::Read(self.layout(segment)[capture.index() - builder.len()].1),
        }
    }

    /// Whether a frame of the callable `segment` may be born from anywhere: it is written in place,
    /// born in a `MODULE` or `GROUP` body, or named as a value anywhere but a call's head.
    pub fn escapes(&self, segment: Segment) -> bool {
        self.tails[segment.index() as usize].escapes
    }

    /// The parameters of the callable `segment` a tail edge from it into itself passes anything but
    /// that same parameter at, sorted.
    pub fn rebound(&self, segment: Segment) -> &[Slot] {
        self.tails[segment.index() as usize].rebound
    }

    /// The parameters of the callable `segment` every tail edge into it passes on bare, sorted:
    /// empty where it escapes.
    pub fn invariant(&self, segment: Segment) -> &[Slot] {
        self.tails[segment.index() as usize].invariant
    }
}

impl<'graph> Shapes<'graph> {
    /// How many of `segment`'s slots keep a digest memo.
    pub fn digested_count(&self, segment: Segment) -> usize {
        self.memos[segment.index() as usize].digested.len()
    }

    /// Where `slot`'s digest memo sits among `segment`'s, if it keeps one.
    pub fn digested_index(&self, segment: Segment, slot: Slot) -> Option<usize> {
        (self.memos[segment.index() as usize].digested)
            .binary_search(&slot)
            .ok()
    }

    /// Where the captures of the module the `:|` at `site` of `segment` views keep their digest
    /// memos, where it views a module binder read bare.
    pub fn view_slots(&self, segment: Segment, site: Site) -> Option<ViewSlots<'graph>> {
        (self.memos[segment.index() as usize].views)
            .get(site)
            .map(|(_, slots)| *slots)
    }

    /// The digested slots the call of `segment` keyed by `site` passes bare: empty where none.
    pub fn handoffs(&self, segment: Segment, site: Site) -> &'graph [Handoff] {
        (self.memos[segment.index() as usize].handoffs)
            .get(site)
            .map_or(&[], |(_, handed)| *handed)
    }
}
