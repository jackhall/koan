//! Static selection: every value expression and value binder given a **static type** where the
//! program loads, and every keyworded use's candidates narrowed by them.
//!
//! A static type is an [`Interval`]: every type the run carries at that expression lies within it.
//! An exact one is a point. The pass is local and bidirectional over what the
//! type half ([`unit`](crate::elaborate::TypeContext)) fixed — a parameter's declared type
//! is its reads', an ascription's type its own, a callee's declared return its calls', and a
//! literal's, a container's, a construction's and a selected candidate's own type flow up. A
//! parameter or an ascription is exactly its type where the retype makes it so — a list, dict or
//! record type or a nominal one — since the run retypes the value to it, and at most its type
//! otherwise. What the load cannot bound is `[Never, Any]`. A static type may hold the lexical
//! variables of its **chain** — the shapes from the program or a quote's code down, at the levels
//! the shape numbers them at — and a crossing into code, or an `EVAL` leaving it, is read through
//! [`bound_above`], so no variable leaks across. A capture is followed to its source along the
//! chain by the rule the shape builder and the shape table follow it by ([`follow`]), which
//! reports a crossing into code; a candidate it reaches across a code root whose registered shape
//! names a variable of the chain outside is unknown.
//!
//! Each keyworded use **judges** each candidate class by class ([`judge_by_class`]): *never*
//! drops it, *always* means it admits whatever the run carries, and a *maybe* one the call admits.
//! An argument at a slot whose class solves a variable ([`solving_slots`]) **contributes** its
//! static upper end to the solve, unless that is `Any`: the judge reads it as exactly that type, and
//! the use records it for the call. A contribution naming a lexical variable records where the use
//! reads it — a hop per block, and a **type capture** per callable or module between the use and the
//! variable's home, which the shape lays down past its builder's captures; a module body's `OVER`
//! list must name each one it holds, as it names a capture the builder made. A path a shape's type
//! expressions spell, read through a declared application naming a lexical variable, records where
//! the shape reads each one the same way, so the run can fit the module to it.
//!
//! The pass resolves each path variable off its module's content tree ([`paths`]) and reads every
//! fact the type half fixed through those resolutions. A member read through a name chain reads
//! each head parameter its module's signature leaves unpinned as the path through it, so two reads
//! through one module share a type; a path variable a use records is read off a root the reading
//! shape reaches. A module-valued call, ascription or member read whose tree is a view is at most
//! the view's application, and exactly its signature over a closed tree.
//! A use left with none refuses the load. A *maybe* an *always* one outranks at class 0, both
//! closed, drops too. Where no *maybe* is left, a lone candidate, or the one closed candidates rank
//! first, is **selected** and runs without admitting; where they rank none first, the load refuses
//! the certain ambiguity. A builtin candidate is judged through its native's [type rule](super::rules)
//! too: an argument whose lower end lies outside the type the rule needs drops it, and a use whose
//! last candidate a need dropped is refused in the native's words. A builtin's call is the return
//! its rule gives — `FROM`'s the projection of its record and `ATTR`'s the named field's type. An
//! `EVAL` is its declared type, as a registration's call is, and code the load traces to a written
//! quote is checked against it as a body is checked against its return. A
//! quantified candidate's return is read through its group's intervals. A call whose callee and
//! solve the load knows exactly — a selected registration, or a call by name of an exact callee —
//! is exactly the return its frame retypes to; a call in tail position never finishes, and is
//! typed as any other. A callable body whose static type meets its declared return at `Never`
//! refuses the load too, as does an ascription whose operand's static type meets its type at
//! `Never`; one whose operand's static upper end lies under its type is **settled**, and the run
//! checks nothing. An annotated binder, `LET <name> <type> = <value>`, is held to its type as an
//! ascription holds its operand, settled by its type part's site.
//!
//! A call by name of an exact callee is judged as a keyworded use of its lone candidate: the
//! callee's parameters laid out as one class, in symbol order, and its argument's static type laid
//! onto them end by end. It contributes, records its contributions by its argument's site for the
//! frame — one per parameter, as a keyworded use records one per argument — and is refused where
//! the judge finds it *never*, or where its argument can never name the parameters exactly. What
//! the pass fixes rests in the records of the units that fixed it, which
//! [`evaluate`](super::evaluate) reads through [`LoadFacts`](crate::scope::LoadFacts).
//!
//! A node is read here exactly as the evaluator reads it, through its [`Form`]. A shape's code is
//! typed before its statements, so an `EVAL` finds it typed. It is typed twice: once for its records,
//! where an unmarked key's hole is a candidate the load cannot read, since a `USING` may fill it;
//! and once as a traced `EVAL` runs it, unfilled, the hole holding nothing — which fixes nothing.
//! Inside a quote's code a refusal is kept in the code's closing record, and the `EVAL` running it
//! reports it.
//!
//! A `MODULE` or `GROUP` binder's body is typed where its binder is, since it runs inline, and the
//! binder is exactly the signature the run ties where every member is exact ([`module_signature`]),
//! else at most `Module`. A `:|` ascription is a view: refused where every run's
//! view door refuses it — an operand that can never be a module, a closed type that is no one
//! application of a signature — or where an exact operand can never fit it; exactly the view's
//! signature over a tree the load sees all of, and at most the type otherwise. A `:!` is a retype,
//! held to its type as an annotation is; a module exact at a closed type is exactly the
//! application the run pins it at ([`pinned_at_fit`]), and refused where it fits none. A member read `m.f` is typed by its
//! [rule](super::rules), which reads the member off the operand's signature, and refused where a
//! signature its lower end is lacks the member. A `USING … SCOPE` body is typed as a block, each
//! surfaced name at the member read it names; a key it surfaces is a spread typed by the one head
//! its operand declares there, at most *maybe*, since a view carries every overload its member
//! admits.
//!
//! A static type is over [`Parametric`] types: it may hold the lexical variables of its chain. A
//! binder of a quantified callable is typed by its scheme — a module body's plain `LET` member, or a
//! keyworded form's name — and a call's head reads it as a [`DeclaredType`]. Anywhere else a
//! quantified function is an **instance site**: a part is typed with the type it is **wanted** at —
//! an annotation's, an ascription's, a callable body's declared return for its last statement, a
//! container's element type where that is the container's own type, a call by name's parameter
//! record, and an argument's slot at each candidate — and the site is instantiated at the least
//! instance under it ([`instance_under`]). A member read of a quantified member is a site too,
//! where its operand is a name or a chain of member reads rooted at one; one over a head parameter
//! its signature leaves unpinned is refused there, and read only at a call's head, whose site the
//! instance table records as read as it is ([`InstanceRead::AsIs`]). A quantified callee's other
//! arguments solve its group first, each read as the solve reads it — a contributing one exactly at
//! its upper end — and the slot is read through that solve: a variable a class before the slot's
//! solves is taken from that class's solving slots at their contributions, as the call solves it —
//! class by class, each pinned to what the classes before it solved — where a run reproduces the
//! solve.
//! The candidates a use keeps must agree on each instance. A name's solution is recorded by its
//! site and a literal's in its body's born-instance cell; a site the wanted type fixes nothing at
//! refuses the load. A solution naming a lexical variable records where the site reads it, as a
//! contribution does, and the run reads the type it binds there. So a part's and a statement's
//! static type is never a scheme.
//!
//! The pass classifies every **frame body** — the program, a callable, a quote's code — as it
//! closes the body, the arms and module bodies it runs counted with it
//! ([type tiers](../../design/type-tiers.md#two-classes-of-body)). A value is porous where its
//! static type's upper end is porous or exposing, and, where that type is exact and closed, where it
//! is exposing ([`porous_site`]). A body is [porous](BodyClass::Porous) where a porous value leaves
//! it at a porous slot: its last statement at its declared return — a quote's code read as
//! returning `Any` — or an argument of a call it makes into a frame: a registration, a function
//! called by name, a surfaced head, a candidate the load cannot read, or any `EVAL`. A value that
//! holds nothing the body minted is no exit, at a call or at the return
//! ([`holds_nothing_minted`](Pass::holds_nothing_minted)): one at a site that is not porous; one
//! the body received — a parameter, a capture or a `LET` bound bare to one, named bare, or a member
//! of one read through the builtin `ATTR`; a record literal written in place whose every field holds nothing minted; or a
//! module whose content tree reaches no view, no closure, no module body holding a minting site
//! and no leaf but a value the body received. A builtin's
//! native runs in the caller's step and enters no frame. Any other body is sealed, and caches
//! where it is a callable or a quote's code that mints at run ([`caching`]). The walk records each
//! `:|` ascription and each component of nominal declarations the load leaves unknown as a
//! **minting site**, which the run's doors check in debug builds.
//!
//! The pass reads each `:|` ascription's operand as a [content tree](crate::scope::Tree) —
//! [`trees`] — and records the view's tree by the ascription's site. Over a closed tree it
//! lays every type the view door computes from types alone
//! ([`opaque_view_types`](view::opaque_view_types)) in the root, each nested module's re-view among
//! them, read off what the tree shows each member carries ([`views`]); at an exact `:!`, the types
//! a member read through the header reads. The run's door checks in debug builds that the tree
//! reads the view's digest.
//!
//! This file is the walk's driver: it enters and leaves each body and unit, and lays each unit's
//! record in the store as the unit ends. Every fact a rule reads or writes goes through the façade,
//! [`reads`]; the files here but the façade hold the walk's **rules**, which lay every value they
//! compute in the walk's scratch and never ask which store they run over — a load's or a
//! resumption's. `tests::boundary` holds them to that.
//!
//! Under test, `unnarrowed` loads a program with every keyworded use left whole and none refused,
//! each contribution still recorded: the narrowing law compares a run so loaded with the narrowed
//! one.
//!
//! See [README.md § Static types](README.md#static-types).

use crate::elaborate::{SignatureMember, declares_nominal, module_signature, signature_member};
use crate::knot::module::view::{self, Ascription, ViewTypes, opaque_view_types};
use crate::knot::{BuiltinFunction, KBuiltins};
use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect, resident};
use crate::parse::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::scope::{
    Address, BodyClass, BodyShape, Born, BuiltinIndex, Candidate, CandidateList, CaptureChain,
    CaptureSlot, CaptureSource, Closing, ComponentIndex, Coordinate, Frontier, InstanceRead, Keyed,
    Lands, Leaf, Listed, NamedVariable, Narrowing, PathEntry, Position, Print, Resolution, Segment,
    ShapeError, ShapeKind, Shapes, Site, SiteDeps, Slot, Source, Static, StaticRegistered,
    StaticSolution, StaticType, Store, SurfacedHead, Tree, Typed, UnitFacts, UnitId, UnitIndex,
    UnitTable, UnitWork, Unknown, UseStatics, Variable as Located, Within, follow, part_of,
    source_of,
};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{
    Collector, DeclaredType, DispatchTokenElement, InstanceFailure, Interval, KKind, KType,
    Parametric, Scheme, Side, TypeNode, Variable, Variance, Verdict, admits_with, bound_above,
    fits, instance_under, instantiate_quantified, intervals, judge_by_class, meet, read_through,
    scheme_bound_above, scheme_return, scheme_slots, select_by_class, shape_return, shape_slots,
    solving_slots,
};
use crate::values::{
    Chain, ConstructionRefused, Value, construction, dict_type, list_type, pinned_at_fit,
    record_type, retype_target, retyped_to, under, unknown,
};

mod caching;
mod paths;
mod reads;
use paths::exact;
use reads::Part;
mod trees;
mod types;
mod views;

use trees::Written;

use super::builtins::Native;
use super::evaluate::{Form, Wanted, of_node, of_part, slots};
use super::one_name;
use super::rules::{self, Given, Lacks, member_read};
use super::select;

/// Walk every body of `shapes` into `store`, interning through `types`: the language's load, or a
/// resumption under the store's binding. A refusal of written code stops the walk, worded by the
/// store.
pub(super) fn walk<'graph: 'o, 'tier: 'o, 'o, S: Store<'o>>(
    shapes: &'graph Shapes<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    store: &S,
    types: &Chain<'tier, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), ShapeError<'o>> {
    let count = shapes.bodies().len();
    let mut pass = Pass {
        shapes,
        builtins,
        types,
        scratch,
        store,
        lay: store.lay(),
        chain: BumpVec::new_in(scratch),
        units: BumpVec::new_in(scratch),
        started: UnitTable::new(shapes, scratch),
        open: BumpVec::new_in(scratch),
        scopes: std::cell::Cell::new(BumpVec::new_in(scratch)),
        walked: scratch.alloc_slice_fill_with(count, |_| false),
        demand: Demand {
            marks: scratch.alloc_slice_fill_with(count, |_| None),
            stack: BumpVec::new_in(scratch),
            next: 0,
            pending: BumpVec::new_in(scratch),
        },
        composed: scratch.alloc_slice_fill_with(count, |_| BumpVec::new_in(scratch)),
        unfilled: false,
        unnarrowed: unnarrowed_now(),
        trees: bump_table(scratch),
        reading: BumpVec::new_in(scratch),
        views: bump_table(scratch),
        steps: bump_table(scratch),
        rebuilds: bump_table(scratch),
        modules: bump_table(scratch),
        paths: BumpVec::new_in(scratch),
        composites: bump_table(scratch),
        module_mints: bump_table(scratch),
        unlisted: None,
        open_path: None,
        components: BumpVec::new_in(scratch),
    };
    pass.body(None, shapes.root())
}

/// A binder's static type: a type's interval, or a quantified callable's scheme.
type Bound = DeclaredType<Interval>;

/// `entries`, written in key order, laid through `lay` as a keyed table.
fn keyed<'o, K: Ord + Copy, V: Copy>(lay: impl Lays<'o>, entries: &[(K, V)]) -> Keyed<'o, (K, V)> {
    Keyed::from_sorted(collect(lay, entries.iter().copied()))
}

/// Keep the newest of `held`'s records at each site: the latest written last.
fn newest<T>(held: &mut BumpVec<'_, (Site, T)>) {
    // A stable sort keeps the records at one site in order, the newest last.
    held.sort_by_key(|(site, _)| *site);
    held.reverse();
    held.dedup_by_key(|(site, _)| *site);
}

/// `value` and `wanted` read through their bounds, where they meet at `Never`: no value of the
/// static type `value` can ever satisfy `wanted`. `None` where one might. Each end is named
/// through its bound, since a variable's name is its binder's.
fn never_satisfies(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Parametric,
    wanted: Parametric,
) -> Option<(KType, KType)> {
    let (value, wanted) = (
        bound_above(types, scratch, value),
        bound_above(types, scratch, wanted),
    );
    (meet(types, scratch, value, wanted) == KType::NEVER).then_some((value, wanted))
}

/// Whether a value of the static type `typed` can reach a type minted at run: where its upper end
/// is porous or exposing, and, where that is exact and closed, where it is exposing — an exact
/// closed site carries that very type, and the surface door hides what the type does not name.
fn porous_site(types: &Chain<'_, '_, '_, '_>, typed: Interval) -> bool {
    match exact(types, typed) {
        Some(_) => types.exposing(typed.upper),
        None => porous_slot(types, typed.upper),
    }
}

/// Whether a value held to the declared type `slot` — a return, a parameter — can carry a type
/// minted at run.
fn porous_slot(types: &Chain<'_, '_, '_, '_>, slot: Parametric) -> bool {
    types.porous(slot) || types.exposing(slot)
}

/// Where a coordinate lands, its captures followed to their sources.
#[derive(Clone, Copy)]
enum Landing {
    Builtin(BuiltinIndex),
    /// A slot of the shape at `level`: a local, or the knot member a capture names.
    Slot {
        level: usize,
        slot: Slot,
    },
    /// A capture whose source the chain does not hold: a hole, a name offered, or a read past the
    /// chain's first shape.
    Open {
        hole: bool,
    },
}

/// A landing, and whether the walk to it crossed into a quote's code: took a capture of a shape
/// that roots a chain.
#[derive(Clone, Copy)]
struct Followed {
    landing: Landing,
    crossed: bool,
    /// The capture of a quote's code the walk took into it, or ended at: a hole, a `\` mark or a
    /// `$` name, each a name the code is run with.
    entered: Option<Unknown>,
}

/// A unit of this walk, by index into the walk's units: the one a fact the walk holds was computed
/// in, which a read of the fact gathers the frontier of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Tag(u32);

/// One unit the walk ran or absorbed, or one owning body's closing.
struct UnitState<'p, 'o> {
    id: UnitId,
    /// Once it ends: its record's frontier. While it runs, its gathering is the walk's.
    frontier: Option<Frontier<'o>>,
    /// What its record keeps beside its owner's own facts: each inline body it ran, laid as the
    /// body leaves the chain; each callable or quote's code it births; each path it resolved first
    /// on its chain; what the folds read.
    typed: BumpVec<'p, (Segment, Typed<'o>)>,
    born: BumpVec<'p, (Segment, Born<'o>)>,
    paths: BumpVec<'p, ((Segment, usize), (PathEntry<'o>, Resolution<'o>))>,
    sites: BumpVec<'p, SiteDeps<'o>>,
    named_variables: BumpVec<'p, NamedVariable>,
    /// Where it wrote into its owning body's level: what its record lays.
    wrote: BumpVec<'p, Wrote>,
    /// What its facts give its owning body's class, which a resumption absorbing it folds back
    /// in: a porous exit, how many of its minting sites are views over closed trees, and the run
    /// mints of the blocks and module bodies it ran.
    exits: bool,
    closed: u32,
    folded: u32,
}

impl<'p, 'o> UnitState<'p, 'o> {
    /// The state of `id` as it opens — a unit or a body's closing — with `frontier` where it is
    /// absorbed as a record holds it.
    fn new(id: UnitId, frontier: Option<Frontier<'o>>, scratch: BumpAllocator<'p>) -> Self {
        UnitState {
            id,
            frontier,
            typed: BumpVec::new_in(scratch),
            born: BumpVec::new_in(scratch),
            paths: BumpVec::new_in(scratch),
            sites: BumpVec::new_in(scratch),
            named_variables: BumpVec::new_in(scratch),
            wrote: BumpVec::new_in(scratch),
            exits: false,
            closed: 0,
            folded: 0,
        }
    }
}

/// Where a unit wrote a fact into its owning body's [`Level`]: a slot, a statement or a use of the
/// dense tables, a site or a level of the keyed ones, or an index into a list. A unit's record lays
/// what it wrote, never a scan of the whole body.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Wrote {
    Declared(Slot),
    Registered(Slot),
    Binder(Slot),
    Statement(usize),
    Use(usize),
    Part(Site),
    Expression(Site),
    Selection(Site),
    Through(usize),
    Settled(usize),
    Minting(usize),
    Instance(usize),
    Named(usize),
    Tree(usize),
    Content(usize),
}

/// One shape on the chain, innermost last, with what the walk has fixed of it so far, each fact
/// beside the unit that fixed it.
struct Level<'p, 'o, 'graph> {
    shape: &'graph BodyShape<'graph>,
    /// Whether the shape roots a chain: the program or a quote's code.
    roots_chain: bool,
    /// The index in `Pass::paths` of the table of the chain the shape sits on.
    paths: usize,
    /// The type half's facts: each type binder's declaration and each registration's entry, by
    /// slot, and each recorded type expression's type, by site.
    declared: BumpVec<'p, Option<(StaticType<'o>, Tag)>>,
    registered: BumpVec<'p, Option<(StaticRegistered<'o>, Tag)>>,
    expressions: BumpBackedMap<'p, Site, (StaticType<'o>, Tag)>,
    /// Each part's static type, by site: its latest where the walk typed it twice.
    parts: BumpBackedMap<'p, Site, (Interval, Tag)>,
    statements: BumpVec<'p, Option<(Interval, Tag)>>,
    binders: BumpVec<'p, (Bound, Option<Tag>)>,
    /// What the walk fixed about each keyworded use, by index into the shape's candidate lists.
    uses: BumpVec<'p, (UseStatics<'o>, Option<Tag>)>,
    /// Each keyworded use the walk selected a candidate at, by its node's site, beside the closed
    /// point each variable of the candidate's group solves to there, by group index.
    selections: BumpBackedMap<'p, Site, (&'o [Option<KType>], Tag)>,
    /// Each `:!` whose operand's static upper end lies under its type, and each annotated binder's
    /// type part whose value's does.
    settled: BumpVec<'p, (Site, Tag)>,
    /// Each site where a quantified function is read, beside what the load fixed there.
    instances: BumpVec<'p, (Site, InstanceRead<'o>, Tag)>,
    /// Each call by name's contributions, by its argument part's site: per parameter in symbol
    /// order.
    named: BumpVec<'p, (Site, &'o [StaticType<'o>], Tag)>,
    /// Whether a porous value leaves the shape: through its return, or an argument of a call it
    /// makes into a frame. A block's and a module body's are folded into the shape enclosing them.
    exits: bool,
    /// The shape's own minting sites.
    minting: BumpVec<'p, (Site, Tag)>,
    /// How many of those are views over a closed content tree, which mint nothing at run.
    closed: u32,
    /// The minting sites of the blocks and module bodies the shape runs, folded in.
    folded: u32,
    /// Whether a block or module body the shape runs holds a minting site, a view over a closed
    /// tree included.
    minted_within: bool,
    /// Each `:|` ascription's site, beside its operand's content tree.
    trees: BumpVec<'p, (Site, &'o Tree<'o>, Tag)>,
    /// Each module binder's content tree, by slot: what a binding of it is read against.
    contents: BumpVec<'p, (Slot, Tree<'o>, Tag)>,
    /// Each path the shape spells whose declared application names a lexical variable, by its
    /// level, beside where the shape reads each variable.
    throughs: BumpBackedMap<'p, usize, (&'o [Located], Tag)>,
    /// A `MODULE` or `GROUP` body's exact signature, where the walk typed one.
    signature: Option<KType>,
    /// The statement the walk is typing: where it looks for a part first.
    statement: usize,
    /// A `MODULE` body's builder captures' trees, resolved once a leaf on one of its binders is
    /// keyed on them.
    captures: Option<&'o [(u32, Tree<'o>)]>,
}

impl Level<'_, '_, '_> {
    /// Where every fact this level holds sits, whichever unit wrote it: what an inline body lays
    /// whole as it leaves the chain.
    fn everything<'x>(&self, scratch: BumpAllocator<'x>) -> BumpVec<'x, Wrote> {
        let mut wrote = BumpVec::new_in(scratch);
        for index in 0..self.shape.slots() {
            let slot = Slot(index as u32);
            wrote.extend([
                Wrote::Declared(slot),
                Wrote::Registered(slot),
                Wrote::Binder(slot),
            ]);
        }
        wrote.extend((0..self.statements.len()).map(Wrote::Statement));
        wrote.extend((0..self.uses.len()).map(Wrote::Use));
        wrote.extend(self.parts.keys().map(|site| Wrote::Part(*site)));
        wrote.extend(self.expressions.keys().map(|site| Wrote::Expression(*site)));
        wrote.extend(self.selections.keys().map(|site| Wrote::Selection(*site)));
        wrote.extend(self.throughs.keys().map(|level| Wrote::Through(*level)));
        wrote.extend((0..self.settled.len()).map(Wrote::Settled));
        wrote.extend((0..self.minting.len()).map(Wrote::Minting));
        wrote.extend((0..self.instances.len()).map(Wrote::Instance));
        wrote.extend((0..self.named.len()).map(Wrote::Named));
        wrote.extend((0..self.trees.len()).map(Wrote::Tree));
        wrote.extend((0..self.contents.len()).map(Wrote::Content));
        wrote
    }

    /// The sites this shape and the blocks and module bodies it runs mint at at run: every minting
    /// site but a view over a closed content tree.
    fn run_mints(&self) -> u32 {
        self.minting.len() as u32 - self.closed + self.folded
    }
}

/// Tarjan's bookkeeping over the callable bodies the walk types, so a call into a body still
/// being typed — itself, or one of a cycle of them — never unfolds, whatever order the bodies are
/// walked in.
struct Demand<'p, 'o> {
    /// Each callable's mark, by segment, once the walk enters it.
    marks: &'p mut [Option<Mark>],
    /// Each callable entered and not yet in a finished component, innermost last.
    stack: BumpVec<'p, Segment>,
    next: u32,
    /// Each closing computed whose component is not finished yet, beside its frontier.
    pending: BumpVec<'p, (Segment, Closing<'o>, Frontier<'o>)>,
}

/// One callable's place in [`Demand`].
#[derive(Clone, Copy)]
struct Mark {
    index: u32,
    low: u32,
    on_stack: bool,
    /// Whether a kept call of the body reaches the body itself.
    self_edge: bool,
}

/// The walk's state: the chain of enclosing shapes. Every value a record keeps is laid through
/// `lay`, at `'o`, and every other in `scratch`, at `'p`; every type is interned through `types`.
struct Pass<'p, 'o, 'x, 'graph, 'tier, 'pair, 'reg, S: Store<'o>> {
    shapes: &'graph Shapes<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    types: &'x Chain<'tier, 'pair, 'reg, 'x>,
    scratch: BumpAllocator<'p>,
    store: &'x S,
    lay: S::Lay,
    chain: BumpVec<'p, Level<'p, 'o, 'graph>>,
    /// Every unit the walk ran or absorbed, and every closing, by tag.
    units: BumpVec<'p, UnitState<'p, 'o>>,
    /// The tag of each unit the walk ran or absorbed, by id: the newest where it has two.
    started: UnitTable<'p, Tag>,
    /// The units running, innermost last, beside each one's gathering: every unknown its facts
    /// read so far, unsorted until its frontier lays it.
    open: BumpVec<'p, Tag>,
    scopes: std::cell::Cell<BumpVec<'p, BumpVec<'p, Unknown>>>,
    /// Each owning body the walk has entered, by segment.
    walked: &'p mut [bool],
    demand: Demand<'p, 'o>,
    /// The levels of the type captures each body composes so far, by segment: a lexical variable a
    /// contribution or a solution names, read from inside the body and homed outside it.
    composed: &'p mut [BumpVec<'p, usize>],
    /// Whether the walk is typing a quote's code as a traced `EVAL` runs it, no hole filled: it
    /// fixes nothing.
    unfilled: bool,
    /// Whether the walk leaves every keyworded use whole and refuses none: the narrowing law's
    /// switch.
    unnarrowed: bool,
    /// Each content tree read on the chain, by its shape's address and where it is written.
    trees: BumpBackedMap<'p, (usize, Written), (Tree<'o>, Tag)>,
    /// Each binder whose tree is being read, by its shape's address: a binder read through itself
    /// reads as a leaf.
    reading: BumpVec<'p, (usize, Slot)>,
    /// What each view over a closed tree binds, by its print: a chain of views reads each once.
    views: BumpBackedMap<'p, Print, (Option<ViewTypes<'o>>, Tag)>,
    /// What each read of a value a lay may reach twice yields, so a chain of views reads each
    /// member once.
    steps: BumpBackedMap<'p, (views::Identity, views::Step), Option<views::Carried<'o>>>,
    /// Each value a view rebuilt, by what it was rebuilt from.
    rebuilds: BumpBackedMap<'p, views::Rebuild, views::Carried<'o>>,
    /// The modules each rebuilt union of cells' signatures may be, cell by cell.
    modules: BumpBackedMap<'p, views::Identity, &'o [views::Carried<'o>]>,
    /// The path table of each chain being typed, outermost first.
    paths: BumpVec<'p, paths::ChainPaths<'p, 'o, 'graph>>,
    /// Each composite site's dependencies, by its site: a part that is one takes its class.
    composites: BumpBackedMap<'p, Site, caching::Reads<'o>>,
    /// Each `MODULE` or `GROUP` body the walk typed, by address: whether it, or a block or module
    /// body it runs, holds a minting site.
    module_mints: BumpBackedMap<'p, usize, bool>,
    /// The first `MODULE` body found composing a type its `OVER` list does not name, beside the
    /// refusal it gives where it is typed.
    unlisted: Option<(Segment, ShapeError<'o>)>,
    /// Where the root of the path the walk last resolved is written, where it is written in place
    /// and the path resolved to a class.
    open_path: Option<SourceRef>,
    /// Each component being typed, outermost first, by its shape's level and its members: a
    /// fellow's callable is not typed on demand while its component is, since its body reads the
    /// fellows' facts, not their seeds.
    components: BumpVec<'p, (usize, &'graph [Slot])>,
}

/// What the load knows of one candidate's registered expression shape.
#[derive(Clone, Copy)]
enum Known {
    /// The same at every run.
    Closed(DeclaredType<KType>),
    /// Over lexical variables the run supplies.
    Rigid(DeclaredType<Parametric>),
    Unknown,
}

impl Known {
    /// The shape, where the load knows it.
    fn shape(self) -> Option<DeclaredType<Parametric>> {
        match self {
            Known::Closed(shape) => Some(shape.into()),
            Known::Rigid(shape) => Some(shape),
            Known::Unknown => None,
        }
    }
}

/// One candidate a keyworded use kept: what the load knows of it, its verdict, its group's
/// intervals where the static solve succeeded, and a builtin's return as its rule gives it.
#[derive(Clone, Copy)]
struct Judgement<'x> {
    candidate: Candidate,
    known: Known,
    /// Whether the candidate is a `USING … SCOPE` block's surfaced head whose return is at most:
    /// any but a closed signature head at a type.
    surfaced: bool,
    verdict: Verdict,
    intervals: Option<&'x [Interval]>,
    ruled: Option<Interval>,
    /// The instance each of the use's instance arguments takes at this candidate, in argument
    /// order.
    instances: &'x [Made<'x>],
}

/// An instance: its exact static type beside the solution it is made at, scratch-lived until its
/// site is recorded.
type Made<'x> = (Interval, &'x [Parametric]);

/// Where an instance site is written: a name read there, a quantified `FN` written in place, whose
/// body the instance is born from, or a member read of a quantified member.
#[derive(Clone, Copy)]
enum Instanced<'graph> {
    Name(&'graph ExpressionPart<'graph>),
    Literal(&'graph BodyShape<'graph>),
    /// A member read, `m.f`, of a quantified member.
    Member(&'graph KExpression<'graph>),
}

/// An instance argument of a keyworded use: its slot's position, its part as written, where its
/// instance is made, its scheme and where it is written.
#[derive(Clone, Copy)]
struct InstanceArgument<'graph> {
    position: usize,
    part: &'graph ExpressionPart<'graph>,
    instanced: Instanced<'graph>,
    scheme: Scheme,
    at: SourceRef,
}

/// Why a quantified callee's other arguments give an instance argument's slot no wanted type.
enum Unsolved<'p> {
    /// One can never fit its own slot: the call can never be admitted.
    Misfit,
    /// They leave these variables of the group the slot names open.
    Open(BumpVec<'p, usize>),
}

/// A call by name's argument laid onto its callee's slots: the argument's static upper end, each
/// slot's static type, and each slot's contribution.
type Laid<'p, 'o> = (Parametric, &'p [Interval], &'o [StaticType<'o>]);

/// A call by name's callee, read as a keyworded use's lone candidate.
#[derive(Clone, Copy)]
struct Callee<'p> {
    /// The callee's type as the load reads it — its function type or scheme — which a refusal
    /// names.
    function: DeclaredType<Parametric>,
    /// An expression shape over the callee's parameters, one slot each in symbol order, every slot
    /// in class 0, returning its return: a scheme over the callee's group where it has one.
    shape: DeclaredType<Parametric>,
    /// The parameters' names in symbol order: slot `i` is `names[i]`'s.
    names: &'p [BinderSymbol],
    /// The type the argument is wanted at: an unquantified callee's parameter record.
    wanted: Option<Parametric>,
    /// Whether the load knows the callee exactly — a scheme, or a point at an unquantified
    /// function type. Only an exact callee is judged and records contributions: one known at most
    /// may bind a function admitting more, or naming other parameters.
    exact: bool,
}

// The walk's chain, by level: a capture past its first shape is one the chain's root is run with.
impl<'o, 'graph: 'o, S: Store<'o>> CaptureChain for Pass<'_, 'o, '_, 'graph, '_, '_, '_, S> {
    type At = usize;

    fn outward(&self, level: usize, hops: u32) -> Option<usize> {
        level.checked_sub(hops as usize)
    }

    fn capture_source(&self, level: usize, capture: CaptureSlot) -> CaptureSource {
        self.shapes
            .capture_source(self.chain[level].shape.segment(), capture)
    }

    fn member(&self, level: usize, component: ComponentIndex, index: u32) -> Slot {
        self.chain[level].shape.components()[component.index()].members[index as usize]
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// The shape the load built at `segment`.
    fn shape_at(&self, segment: Segment) -> &'graph BodyShape<'graph> {
        self.shapes.shape_at(segment)
    }

    fn push(&mut self, shape: &'graph BodyShape<'graph>, roots_chain: bool) {
        let scratch = self.scratch;
        let mut statements = BumpVec::with_capacity_in(shape.body().len(), scratch);
        statements.resize(shape.body().len(), None);
        let mut binders = BumpVec::with_capacity_in(shape.slots(), scratch);
        binders.resize(shape.slots(), (DeclaredType::Type(unknown()), None));
        let mut declared = BumpVec::with_capacity_in(shape.slots(), scratch);
        declared.resize(shape.slots(), None);
        let mut registered = BumpVec::with_capacity_in(shape.slots(), scratch);
        registered.resize(shape.slots(), None);
        let mut uses = BumpVec::with_capacity_in(shape.candidate_lists().len(), scratch);
        uses.resize(shape.candidate_lists().len(), (UseStatics::UNFIXED, None));
        // A chain's root opens its table next; any other shape shares the table of the one below.
        let paths = match roots_chain {
            true => self.paths.len(),
            false => {
                (self.chain.last())
                    .expect("an inline body has an encloser on the chain")
                    .paths
            }
        };
        self.chain.push(Level {
            shape,
            roots_chain,
            paths,
            declared,
            registered,
            expressions: bump_table(scratch),
            parts: bump_table(scratch),
            statements,
            binders,
            uses,
            selections: bump_table(scratch),
            settled: BumpVec::new_in(scratch),
            instances: BumpVec::new_in(scratch),
            named: BumpVec::new_in(scratch),
            exits: false,
            minting: BumpVec::new_in(scratch),
            closed: 0,
            folded: 0,
            minted_within: false,
            trees: BumpVec::new_in(scratch),
            contents: BumpVec::new_in(scratch),
            throughs: bump_table(scratch),
            signature: None,
            statement: 0,
            captures: None,
        });
    }

    /// Walk the owning body `shape` — the program, a callable, or a quote's code — nested in the
    /// shape at chain level `parent`: its entry, each unit and the callables it births, then its
    /// closing record. A quote's code that does not type keeps its refusal in its closing record,
    /// and the walk goes on.
    fn body(
        &mut self,
        parent: Option<usize>,
        shape: &'graph BodyShape<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let segment = shape.segment();
        if std::mem::replace(&mut self.walked[segment.index() as usize], true) {
            return Ok(());
        }
        let code = shape.kind() == ShapeKind::Code;
        let callable = shape.kind() == ShapeKind::Callable;
        if callable {
            self.enter(segment);
        }
        let (depth, open) = (self.chain.len(), self.open.len());
        let roots_chain = parent.is_none() || code;
        self.push(shape, roots_chain);
        let level = self.chain.len() - 1;
        if roots_chain {
            self.open_paths(shape);
        }
        // A quote whose code could not be built has nothing to type.
        let walked = match code && shape.refusal().is_some() {
            true => Err(*shape
                .refusal()
                .expect("a refused code shape holds its refusal")),
            false => self.units_of(level).and_then(|_| self.closing(level)),
        };
        if roots_chain {
            self.close_paths(shape);
        }
        self.chain.truncate(depth);
        let (closing, frontier) = match walked {
            Ok((mut closing, tag)) => {
                // A quote's code is typed a second time as a traced `EVAL` runs it, past its first.
                if code && parent.is_some() {
                    closing.ran = self.ran(shape);
                }
                (closing, self.end(tag))
            }
            Err(error) if code => {
                // A refused quote's rules closed nothing.
                self.unwind(open);
                let refused = Closing {
                    class: None,
                    summary: None,
                    recursive: false,
                    ran: None,
                    refusal: Some(resident(self.lay, error)),
                };
                (refused, Frontier::EMPTY)
            }
            Err(error) => {
                self.unwind(open);
                return Err(error);
            }
        };
        self.leave(shape);
        match callable {
            true => self.finish(segment, closing, frontier),
            false => self.write_closing(segment, closing, frontier),
        }
        Ok(())
    }

    /// Run the entry and each unit of the owning body at `level`, each callable a unit births
    /// walked right after it.
    fn units_of(&mut self, level: usize) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        self.unit(level, UnitIndex::ENTRY, |pass| pass.entry(level, None))?;
        for (index, unit) in shape.units().iter().enumerate() {
            let at = UnitIndex::of_run(index);
            self.unit(level, at, |pass| pass.steps(level, at, unit.work))?;
            for born in self.births(shape, unit.work) {
                if born.kind() == ShapeKind::Callable {
                    self.body(Some(level), born)?;
                }
            }
        }
        Ok(())
    }

    /// Each callable and quote's code the statements of `work` of `shape` birth directly, in
    /// program order.
    fn births(
        &self,
        shape: &'graph BodyShape<'graph>,
        work: UnitWork,
    ) -> BumpVec<'p, &'graph BodyShape<'graph>> {
        let mut born = BumpVec::new_in(self.scratch);
        for index in self.statements(shape, work) {
            born.extend(
                (shape.nested_in(index).iter())
                    .map(|(_, nested)| *nested)
                    .filter(|nested| {
                        matches!(nested.kind(), ShapeKind::Callable | ShapeKind::Code)
                    }),
            );
        }
        born
    }

    /// The statements of `shape` the unit doing `work` runs, in program order.
    fn statements(&self, shape: &BodyShape<'graph>, work: UnitWork) -> BumpVec<'p, usize> {
        let mut statements = BumpVec::new_in(self.scratch);
        match work {
            UnitWork::Statement(index) => statements.push(index as usize),
            UnitWork::Component(index) => {
                statements.extend(
                    (shape.components()[index.index()].members.iter())
                        .filter_map(|member| shape.position_of(*member).statement_index()),
                );
                statements.sort_unstable();
                statements.dedup();
            }
        }
        statements
    }

    /// Run `run` as the unit `index` of the owning body at `level`, inside a gathering of its own,
    /// and write its record; or absorb the record the store holds where its binding does not wake
    /// it. The unfilled typing of a quote's code keeps no record: its reads go to the closing
    /// gathering it runs in.
    fn unit(
        &mut self,
        level: usize,
        index: UnitIndex,
        run: impl FnOnce(&mut Self) -> Result<(), ShapeError<'o>>,
    ) -> Result<(), ShapeError<'o>> {
        if self.unfilled {
            return run(self);
        }
        let id = UnitId {
            segment: self.chain[level].shape.segment(),
            unit: index,
        };
        if let Some((facts, frontier)) = self.store.fact(id)
            && !self.store.wakes(frontier)
        {
            self.absorb(level, id, facts, frontier);
            return Ok(());
        }
        let tag = self.open_unit(id, false);
        run(self)?;
        let frontier = self.end(tag);
        let facts = self.record(level, tag);
        self.store.write(id, facts, frontier);
        Ok(())
    }

    /// Open a gathering for `id`, a unit or — `closing` — a body's closing.
    fn open_unit(&mut self, id: UnitId, closing: bool) -> Tag {
        let scratch = self.scratch;
        let tag = Tag(self.units.len() as u32);
        self.units.push(UnitState::new(id, None, scratch));
        if !closing {
            self.started.set(id, tag);
        }
        self.open.push(tag);
        let mut scopes = self.scopes.replace(BumpVec::new_in(scratch));
        scopes.push(BumpVec::new_in(scratch));
        self.scopes.set(scopes);
        tag
    }

    /// End the innermost gathering, `tag`'s: its frontier, laid down.
    fn end(&mut self, tag: Tag) -> Frontier<'o> {
        let innermost = self.open.pop();
        debug_assert_eq!(innermost, Some(tag), "gatherings end innermost first");
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        let mut gathered = scopes.pop().expect("a running unit has its gathering");
        self.scopes.set(scopes);
        let frontier = Frontier::laid(self.lay, &mut gathered);
        self.units[tag.0 as usize].frontier = Some(frontier);
        frontier
    }

    /// Unwind the gatherings opened past `depth`: a refused quote's rules closed nothing.
    fn unwind(&mut self, depth: usize) {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        while self.open.len() > depth {
            let tag = self.open.pop().expect("a gathering is open");
            scopes.pop();
            self.units[tag.0 as usize].frontier = Some(Frontier::EMPTY);
        }
        self.scopes.set(scopes);
    }

    /// The record of the unit `tag` of the owning body at `level`, once it has ended: the body's
    /// own facts the unit fixed, beside what the unit kept as it ran.
    fn record(&mut self, level: usize, tag: Tag) -> &'o UnitFacts<'o> {
        let lay = self.lay;
        let own = self.typed(level, Some(tag));
        let state = &mut self.units[tag.0 as usize];
        let mut typed = BumpVec::with_capacity_in(state.typed.len() + 1, self.scratch);
        typed.push((self.chain[level].shape.segment(), own));
        typed.extend(state.typed.iter().copied());
        let facts = UnitFacts {
            typed: Keyed::sorted(lay, &mut typed),
            born: Keyed::sorted(lay, &mut state.born),
            paths: Keyed::sorted(lay, &mut state.paths),
            sites: collect(lay, state.sites.iter().copied()),
            named_variables: collect(lay, state.named_variables.iter().copied()),
        };
        resident(lay, facts)
    }

    /// The facts of the shape at `level` the unit `tag` fixed, laid down, from where it wrote them:
    /// every fact the level holds where `tag` is `None`. The writes sort by kind, then by key, so
    /// each table keyed as its writes are comes out sorted.
    fn typed(&mut self, level: usize, tag: Option<Tag>) -> Typed<'o> {
        let (lay, scratch) = (self.lay, self.scratch);
        let mut wrote = match tag {
            Some(tag) => std::mem::replace(
                &mut self.units[tag.0 as usize].wrote,
                BumpVec::new_in(scratch),
            ),
            None => self.chain[level].everything(scratch),
        };
        wrote.sort_unstable();
        wrote.dedup();
        let (exits, closed, folded) = match tag {
            Some(tag) => {
                let unit = &self.units[tag.0 as usize];
                (unit.exits, unit.closed, unit.folded)
            }
            None => {
                let at = &self.chain[level];
                (at.exits, at.closed, at.folded)
            }
        };
        let at = &self.chain[level];
        let ours = |held: Tag| tag.is_none_or(|tag| tag == held);
        let lists = at.shape.candidate_lists();
        let mut declared = BumpVec::new_in(scratch);
        let mut registered = BumpVec::new_in(scratch);
        let mut binders = BumpVec::new_in(scratch);
        let mut statements = BumpVec::new_in(scratch);
        let mut uses = BumpVec::new_in(scratch);
        let mut parts = BumpVec::new_in(scratch);
        let mut expressions = BumpVec::new_in(scratch);
        let mut selections = BumpVec::new_in(scratch);
        let mut throughs = BumpVec::new_in(scratch);
        let mut settled = BumpVec::new_in(scratch);
        let mut minting = BumpVec::new_in(scratch);
        let mut instances = BumpVec::new_in(scratch);
        let mut named = BumpVec::new_in(scratch);
        let mut trees = BumpVec::new_in(scratch);
        let mut contents = BumpVec::new_in(scratch);
        for wrote in wrote.iter().copied() {
            match wrote {
                Wrote::Declared(slot) => {
                    if let Some((typed, held)) = at.declared[slot.index()]
                        && ours(held)
                    {
                        declared.push((slot, typed));
                    }
                }
                Wrote::Registered(slot) => {
                    if let Some((typed, held)) = at.registered[slot.index()]
                        && ours(held)
                    {
                        registered.push((slot, typed));
                    }
                }
                Wrote::Binder(slot) => {
                    if let (typed, Some(held)) = at.binders[slot.index()]
                        && ours(held)
                    {
                        binders.push((slot, typed));
                    }
                }
                Wrote::Statement(index) => {
                    if let Some((typed, held)) = at.statements[index]
                        && ours(held)
                    {
                        statements.push((index as u32, typed));
                    }
                }
                Wrote::Use(index) => {
                    if let (fixed, Some(held)) = at.uses[index]
                        && ours(held)
                        && !fixed.is_unfixed()
                    {
                        uses.push((lists[index].0, fixed));
                    }
                }
                Wrote::Part(site) => {
                    if let Some((typed, held)) = at.parts.get(&site)
                        && ours(*held)
                    {
                        parts.push((site, *typed));
                    }
                }
                Wrote::Expression(site) => {
                    if let Some((typed, held)) = at.expressions.get(&site)
                        && ours(*held)
                    {
                        expressions.push((site, *typed));
                    }
                }
                Wrote::Selection(site) => {
                    if let Some((points, held)) = at.selections.get(&site)
                        && ours(*held)
                    {
                        selections.push((site, *points));
                    }
                }
                Wrote::Through(path_level) => {
                    if let Some((located, held)) = at.throughs.get(&path_level)
                        && ours(*held)
                    {
                        throughs.push((path_level, *located));
                    }
                }
                Wrote::Settled(index) => {
                    let (site, held) = at.settled[index];
                    if ours(held) {
                        settled.push(site);
                    }
                }
                Wrote::Minting(index) => {
                    let (site, held) = at.minting[index];
                    if ours(held) {
                        minting.push(site);
                    }
                }
                Wrote::Instance(index) => {
                    let (site, read, held) = at.instances[index];
                    if ours(held) {
                        instances.push((site, read));
                    }
                }
                Wrote::Named(index) => {
                    let (site, read, held) = at.named[index];
                    if ours(held) {
                        named.push((site, read));
                    }
                }
                Wrote::Tree(index) => {
                    let (site, tree, held) = at.trees[index];
                    if ours(held) {
                        trees.push((site, tree));
                    }
                }
                Wrote::Content(index) => {
                    let (slot, tree, held) = at.contents[index];
                    if ours(held) {
                        contents.push((slot, tree));
                    }
                }
            }
        }
        let sites = |sites: &mut BumpVec<'_, Site>| {
            sites.sort_unstable();
            sites.dedup();
            Keyed::from_sorted(collect(lay, sites.iter().copied()))
        };
        let (settled, minting) = (sites(&mut settled), sites(&mut minting));
        newest(&mut instances);
        newest(&mut named);
        newest(&mut trees);
        Typed {
            declared: keyed(lay, &declared),
            registered: keyed(lay, &registered),
            expressions: keyed(lay, &expressions),
            parts: keyed(lay, &parts),
            statements: keyed(lay, &statements),
            binders: keyed(lay, &binders),
            settled,
            instances: Keyed::sorted(lay, &mut instances),
            uses: Keyed::sorted(lay, &mut uses),
            selections: keyed(lay, &selections),
            by_name: Keyed::sorted(lay, &mut named),
            trees: Keyed::sorted(lay, &mut trees),
            contents: Keyed::sorted(lay, &mut contents),
            minting,
            throughs: keyed(lay, &throughs),
            signature: at.signature,
            exits,
            closed,
            folded,
        }
    }

    /// Absorb the record of `id`, a unit of the owning body at `level` the store holds and its
    /// binding does not wake: the body's own facts into its level, beside the unit's frontier, and
    /// the paths it resolved into its chain's table. Every callable it births is walked still,
    /// through the inline bodies holding it.
    fn absorb(
        &mut self,
        level: usize,
        id: UnitId,
        facts: &'o UnitFacts<'o>,
        frontier: Frontier<'o>,
    ) {
        let tag = Tag(self.units.len() as u32);
        self.units
            .push(UnitState::new(id, Some(frontier), self.scratch));
        self.started.set(id, tag);
        let segment = self.chain[level].shape.segment();
        if let Some((_, typed)) = facts.typed.get(segment) {
            self.restore(level, typed, tag);
        }
        for ((root, _), (entry, resolution)) in facts.paths.iter() {
            self.absorb_path(*root, *entry, *resolution, tag);
        }
    }

    /// Put `typed`, facts a record holds for the shape at `level`, back into its level as fixed
    /// by the unit `tag`.
    fn restore(&mut self, level: usize, typed: &Typed<'o>, tag: Tag) {
        let at = &mut self.chain[level];
        let lists = at.shape.candidate_lists();
        for (slot, typed) in typed.declared.iter() {
            at.declared[slot.index()] = Some((*typed, tag));
        }
        for (slot, typed) in typed.registered.iter() {
            at.registered[slot.index()] = Some((*typed, tag));
        }
        at.expressions.extend(
            typed
                .expressions
                .iter()
                .map(|(site, typed)| (*site, (*typed, tag))),
        );
        at.parts.extend(
            typed
                .parts
                .iter()
                .map(|(site, typed)| (*site, (*typed, tag))),
        );
        for (index, typed) in typed.statements.iter() {
            at.statements[*index as usize] = Some((*typed, tag));
        }
        for (slot, typed) in typed.binders.iter() {
            at.binders[slot.index()] = (*typed, Some(tag));
        }
        for (site, fixed) in typed.uses.iter() {
            if let Some(index) = lists.position(*site) {
                at.uses[index] = (*fixed, Some(tag));
            }
        }
        for (site, points) in typed.selections.iter() {
            at.selections.insert(*site, (*points, tag));
        }
        at.settled
            .extend(typed.settled.iter().map(|site| (*site, tag)));
        at.instances.extend(
            typed
                .instances
                .iter()
                .map(|(site, read)| (*site, *read, tag)),
        );
        at.named
            .extend(typed.by_name.iter().map(|(site, read)| (*site, *read, tag)));
        at.minting
            .extend(typed.minting.iter().map(|site| (*site, tag)));
        at.trees
            .extend(typed.trees.iter().map(|(site, tree)| (*site, *tree, tag)));
        for (level, located) in typed.throughs.iter() {
            at.throughs.entry(*level).or_insert((*located, tag));
        }
        at.exits |= typed.exits;
        at.closed += typed.closed;
        at.folded += typed.folded;
    }

    /// The unit running, whose record keeps what the walk fixes now: none while an unfilled
    /// typing fixes nothing.
    fn fixing(&mut self) -> Option<&mut UnitState<'p, 'o>> {
        if self.unfilled {
            return None;
        }
        let tag = *self.open.last()?;
        Some(&mut self.units[tag.0 as usize])
    }

    /// Mark the shape at `level` with a porous exit: a porous value leaves it.
    fn exit(&mut self, level: usize) {
        self.chain[level].exits = true;
        if let Some(unit) = self.fixing() {
            unit.exits = true;
        }
    }

    /// The entry of the owning body or inline body at `level`: each slot it binds before any
    /// statement runs seeded — a parameter, its own `FOR ALL` names, its `:Type` parameters, a
    /// block's surfaced names — a `USING … SCOPE` body's surfaced names at the member reads they
    /// name of the module typed `surfacing`.
    fn entry(
        &mut self,
        level: usize,
        surfacing: Option<(Interval, Tag)>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        for slot in shape.holds(UnitIndex::ENTRY).slots.iter().copied() {
            // A surfaced name reads the module's static type, which its `USING` read.
            let surfaced = surfacing.filter(|_| shape.position_of(slot) == Position::PARAMETER);
            if let Some((_, tag)) = surfaced {
                self.gather(tag);
            }
            // A block's surfaced key is a registration bound at its entry.
            if let Some(registration) = shape.registration(slot) {
                self.register(level, registration)?;
            }
            self.seed_slot(level, slot, surfaced.map(|(typed, _)| typed));
        }
        Ok(())
    }

    /// Seed the binder at `slot` of the shape at `level` before its unit types anything.
    fn seed_slot(&mut self, level: usize, slot: Slot, surfacing: Option<Interval>) {
        let seeded = self.seeded(level, slot, surfacing);
        self.note_own(level, slot, seeded);
        self.set_binder(level, slot, seeded);
    }

    /// Run the unit `index` of the shape at `level`, doing `work`, in its five steps: the paths
    /// its statements spell, its declarations, its type expressions and registrations and the
    /// callables it births, its value statics, and its binder facts beside them.
    fn steps(
        &mut self,
        level: usize,
        index: UnitIndex,
        work: UnitWork,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let statements = self.statements(shape, work);
        let holds = shape.holds(index);
        for slot in holds.slots.iter().copied() {
            if !self.component_types(shape, slot) && shape.declarations(slot).is_none() {
                self.seed_slot(level, slot, None);
            }
        }
        // 1. Each path its statements spell, before anything reads it.
        for at in holds.spelled.iter() {
            self.resolve_spelled(level, *at as usize)?;
        }
        // 2. A component of type binders, declared.
        if let UnitWork::Component(component) = work {
            let component = &shape.components()[component.index()];
            if shape.declarations(component.members[0]).is_some() {
                self.declare(level, component)?;
                for member in component.members {
                    self.seed_slot(level, *member, None);
                }
            }
        }
        // 3. Each type expression, in program order, then the guards; each registration it binds;
        //    each callable it births.
        self.type_expressions(level, holds.expressions)?;
        for slot in holds.slots.iter().copied() {
            if let Some(registration) = shape.registration(slot) {
                self.register(level, registration)?;
                self.seed_slot(level, slot, None);
            }
        }
        for at in statements.iter() {
            for (_, nested) in shape.nested_in(*at) {
                if nested.kind() == ShapeKind::Callable {
                    self.birth(level, nested)?;
                }
            }
        }
        // 4 and 5. Each quote's code it births, walked first so an `EVAL` reads it typed; then its
        //    value statics, each binder's fact beside them.
        for at in statements.iter() {
            for (_, nested) in shape.nested_in(*at) {
                if nested.kind() == ShapeKind::Code {
                    self.body(Some(level), nested)?;
                }
            }
        }
        match work {
            UnitWork::Statement(index) => {
                let index = index as usize;
                self.chain[level].statement = index;
                let wanted = self.tail(level, index);
                let typed = self.node(level, &shape.body()[index], wanted)?;
                self.set_statement(level, index, typed);
            }
            UnitWork::Component(index) => self.component(level, index.index())?,
        }
        // A block its value statics did not reach — an arm a builtin's rule reads no further than
        // its value — is typed on its own, inside the unit.
        for at in statements.iter() {
            for (_, nested) in shape.nested_in(*at) {
                if nested.kind() == ShapeKind::Block
                    && !self.walked[nested.segment().index() as usize]
                {
                    self.block(level, nested, None)?;
                }
            }
        }
        // A statement no rule typed is at most `Any`.
        for index in statements.iter() {
            if self.chain[level].statements[*index].is_none() {
                self.set_statement(level, *index, unknown());
            }
        }
        if !self.unfilled {
            self.throughs(level, holds.spelled);
            self.contents(level, index)?;
        }
        Ok(())
    }

    /// 5. Each module binder the unit `index` of the shape at `level` binds: its content tree, which
    ///    a binding of it is read against.
    fn contents(&mut self, level: usize, index: UnitIndex) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        for slot in shape.holds(index).slots.iter().copied() {
            if shape.position_of(slot) == Position::PARAMETER
                || self
                    .module_unknown(level, slot, self.binder_fact(level, slot))
                    .is_none()
            {
                continue;
            }
            let site = match (shape.rhs(slot), shape.birth_site(slot)) {
                (Some(rhs), _) => Site::of(rhs),
                (None, Some(born)) => born,
                (None, None) => continue,
            };
            let tree = self.slot_tree(trees::Scope::Chain(level), slot, site)?;
            self.record_content(level, slot, tree);
        }
        Ok(())
    }

    /// Type the inline body `nested` — an arm, a block, a `USING … SCOPE` body over what it is
    /// `surfacing`, a `MODULE` or `GROUP` body — inside the unit running it, nested in the shape at
    /// `level`: its entry and its units through the same steps, each callable a sub-unit births
    /// walked right after it. Its facts land in the running unit's record. Its last statement's
    /// static type.
    fn inline<T>(
        &mut self,
        level: usize,
        nested: &'graph BodyShape<'graph>,
        surfacing: Option<(Interval, Tag)>,
        finish: impl FnOnce(&mut Self, usize) -> Result<T, ShapeError<'o>>,
    ) -> Result<(T, Level<'p, 'o, 'graph>), ShapeError<'o>> {
        self.push(nested, false);
        let at = level + 1;
        let finished = match self.inline_units(at, surfacing) {
            Ok(()) => finish(self, at),
            Err(error) => Err(error),
        };
        if finished.is_ok() && !self.unfilled {
            let typed = self.typed(at, None);
            self.keep_inline(nested.segment(), typed);
        }
        let popped = self.chain.pop().expect("the body was pushed");
        self.leave(nested);
        self.walked[nested.segment().index() as usize] = true;
        finished.map(|finished| (finished, popped))
    }

    /// Type the block `nested` — an arm, or a `USING … SCOPE` body over what it is `surfacing` —
    /// nested in the shape at `level`, folding its class in: its last statement's static type.
    fn block(
        &mut self,
        level: usize,
        nested: &'graph BodyShape<'graph>,
        surfacing: Option<(Interval, Tag)>,
    ) -> Result<Option<Interval>, ShapeError<'o>> {
        let last = |pass: &mut Self, at: usize| Ok(pass.last_statement(at));
        let (last, popped) = self.inline(level, nested, surfacing, last)?;
        self.fold(level, &popped);
        Ok(last)
    }

    /// The entry and units of the inline body at `level`.
    fn inline_units(
        &mut self,
        level: usize,
        surfacing: Option<(Interval, Tag)>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        self.entry(level, surfacing)?;
        for (index, unit) in shape.units().iter().enumerate() {
            self.steps(level, UnitIndex::of_run(index), unit.work)?;
            for born in self.births(shape, unit.work) {
                if born.kind() == ShapeKind::Callable {
                    self.body(Some(level), born)?;
                }
            }
        }
        Ok(())
    }

    /// Keep `typed`, the facts of the inline body `segment` just typed, in the unit running it: the
    /// newest typing of a body the unit types twice stands.
    fn keep_inline(&mut self, segment: Segment, typed: Typed<'o>) {
        let tag = self.innermost();
        let held = &mut self.units[tag.0 as usize].typed;
        held.retain(|(each, _)| *each != segment);
        held.push((segment, typed));
    }

    /// The tag of the innermost unit running.
    fn innermost(&self) -> Tag {
        *self.open.last().expect("a unit is running")
    }

    /// The static type of a `MODULE` or `GROUP` binder whose body is `body`, nested in the shape at
    /// `level`. The body runs inline before its binder is tied, so it is typed here. Exactly the
    /// signature the run ties ([`module_signature`]) where the load knows every member exactly —
    /// each value binder at a closed point or a closed scheme, each type binder and each
    /// registration closed — and at most `Module` otherwise.
    fn module(
        &mut self,
        level: usize,
        body: &'graph BodyShape<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let signed = |pass: &mut Self, at: usize| {
            if let Some((segment, error)) = pass.unlisted
                && segment == body.segment()
            {
                pass.unlisted = None;
                return Err(error);
            }
            let (typed, signature) = pass.signed(at, body)?;
            if let Some(signature) = signature
                && !pass.unfilled
            {
                pass.chain[at].signature = Some(signature);
            }
            Ok(typed)
        };
        let (typed, popped) = self.inline(level, body, None, signed)?;
        self.fold(level, &popped);
        let mints = !popped.minting.is_empty() || popped.minted_within;
        self.module_mints.insert(address(body), mints);
        Ok(typed)
    }

    /// The exact signature of the `MODULE` or `GROUP` body at `level`, beside its static type.
    fn signed(
        &mut self,
        level: usize,
        body: &'graph BodyShape<'graph>,
    ) -> Result<(Interval, Option<KType>), ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let mut members = BumpVec::with_capacity_in(body.slots(), scratch);
        let mut keyworded = BumpVec::new_in(scratch);
        for index in 0..body.slots() {
            let slot = Slot(index as u32);
            let (bound, _) = self.chain[level].binders[index];
            let exact = match body.slot_name(slot) {
                BinderSymbol::Value(_) => match bound {
                    DeclaredType::Type(typed) if typed.is_exact() => types
                        .concrete(typed.upper)
                        .map(|closed| DeclaredType::Type(closed.into())),
                    DeclaredType::Scheme(scheme) => body
                        .births(slot)
                        .filter(|born| {
                            let typed = self.callable_cell(born);
                            matches!(self.resolved_callable(level, typed), Static::Closed(_))
                        })
                        .map(|_| DeclaredType::Scheme(scheme)),
                    DeclaredType::Type(_) => None,
                },
                BinderSymbol::Type(_) => match self.declared_cell(body, slot) {
                    Static::Closed(held) => Some(DeclaredType::Type(held.into())),
                    _ => None,
                },
                BinderSymbol::Registration(_) => {
                    let typed = self.resolved_registered(level, self.registered_cell(body, slot));
                    let Static::Closed(registered) = typed else {
                        return Ok((under(KType::EMPTY_SIGNATURE.into()), None));
                    };
                    keyworded.push(registered.shape);
                    continue;
                }
                BinderSymbol::Key(_) => unreachable!("no binder declares a key"),
            };
            let Some(exact) = exact else {
                return Ok((under(KType::EMPTY_SIGNATURE.into()), None));
            };
            members.push((slot, exact));
        }
        let signature = module_signature(body, members, &keyworded, types, scratch);
        Ok((Interval::point(signature.into()), Some(signature)))
    }

    /// Fold `inner`, a block or module body the shape at `level` runs, into it: its exits and
    /// its run's minting sites count toward the frame body enclosing it.
    fn fold(&mut self, level: usize, inner: &Level<'p, 'o, 'graph>) {
        let at = &mut self.chain[level];
        at.exits |= inner.exits;
        at.folded += inner.run_mints();
        at.minted_within |= inner.minted_within || !inner.minting.is_empty();
        // Folded into an owning body, the mints are the running unit's; into another inline body,
        // they fold on with it.
        let owning = !matches!(at.shape.kind(), ShapeKind::Block | ShapeKind::Module);
        if owning && let Some(unit) = self.fixing() {
            unit.folded += inner.run_mints();
        }
    }

    /// The class of the shape at `level` where it is a frame body: porous where a porous value
    /// leaves it, and otherwise sealed, caching where it is a callable or a quote's code that
    /// mints at run. The program never caches: its bindings are read across its statements and
    /// nothing above it is parked.
    fn class(&self, level: usize) -> Option<BodyClass> {
        let at = &self.chain[level];
        let mints = at.run_mints();
        let cacheable = match at.shape.kind() {
            ShapeKind::Block | ShapeKind::Module => return None,
            ShapeKind::Program => false,
            ShapeKind::Callable | ShapeKind::Code => true,
        };
        Some(match at.exits {
            true => BodyClass::Porous { mints },
            false => BodyClass::Sealed {
                mints,
                caches: mints > 0 && cacheable,
            },
        })
    }

    /// A callable body's declared return, read off the function type its body runs at, where the
    /// load typed the callable.
    fn declared_return(&self, level: usize) -> Option<Parametric> {
        self.in_body(level, Part::Return)
    }

    /// Mark the shape at `level` porous where its return carries a porous value out: a callable
    /// whose last statement's value is porous and whose declared return — `Any` where the load did
    /// not type it — is too, and a quote's code whose last statement's value is porous, its
    /// `EVAL`'s declared return unknown to the shape. A body that never arrives returns nothing,
    /// and a value [holding nothing it minted](Self::returns_nothing_minted) carries nothing out.
    fn return_exits(&mut self, level: usize) -> Result<(), ShapeError<'o>> {
        let at = &self.chain[level];
        let Some(last) = self.last_statement(level) else {
            return Ok(());
        };
        if last.upper == KType::NEVER.into() {
            return Ok(());
        }
        let types = self.types;
        let slot = match at.shape.kind() {
            ShapeKind::Callable => {
                (self.declared_return(level)).unwrap_or_else(|| KType::ANY.into())
            }
            ShapeKind::Code => KType::ANY.into(),
            ShapeKind::Program | ShapeKind::Module | ShapeKind::Block => return Ok(()),
        };
        if porous_site(types, last)
            && porous_slot(types, slot)
            && !self.returns_nothing_minted(level, last)?
        {
            self.exit(level);
        }
        Ok(())
    }

    /// Whether the last statement of the shape at `level`, of the static type `last`, returns a
    /// value [holding nothing](Self::holds_nothing_minted) the body minted. A binding statement
    /// returns what it binds: a `MODULE` or `GROUP` body's tree, or an unannotated `LET`'s
    /// right-hand side.
    fn returns_nothing_minted(
        &mut self,
        level: usize,
        last: Interval,
    ) -> Result<bool, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let node = shape
            .body()
            .last()
            .expect("a body returning a value has a statement");
        let Some(slot) = trees::last_binder(shape) else {
            let form = of_node(shape, node);
            return self.holds_nothing_minted(
                level,
                form,
                Written::Node(Site::of_node(node)),
                last,
            );
        };
        if let Some(body) = shape.births(slot)
            && body.kind() == ShapeKind::Module
        {
            let scope = trees::Scope::Chain(level);
            let tree = self.slot_tree(scope, slot, Site::of_node(node))?;
            return Ok(!self.reaches_minted(level, &tree));
        }
        match (shape.rhs(slot), shape.annotation(slot)) {
            (Some(rhs), None) => match self.typed_part(level, rhs) {
                Some(typed) => self.holds_nothing_minted_part(level, rhs, typed),
                None => Ok(false),
            },
            _ => Ok(false),
        }
    }

    /// The closing record of the owning body at `level`, its gathering left open for what reads
    /// the body once it leaves the chain: its return check and exits, its class, and a callable's
    /// summary. A module body's `OVER` check runs as its contract is folded.
    fn closing(&mut self, level: usize) -> Result<(Closing<'o>, Tag), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let tag = self.open_unit(
            UnitId {
                segment: shape.segment(),
                unit: UnitIndex::ENTRY,
            },
            true,
        );
        self.returns(level)?;
        self.return_exits(level)?;
        let summary = match shape.kind() {
            ShapeKind::Callable => Some(self.summarize(level)?),
            _ => None,
        };
        let closing = Closing {
            class: self.class(level),
            summary,
            recursive: false,
            ran: None,
            refusal: None,
        };
        Ok((closing, tag))
    }

    /// The quote's code `code`'s last statement's static type as a traced `EVAL` runs it: typed a
    /// second time, on the chain around it, with every unmarked hole empty. It fixes nothing, and
    /// a use it can never run refuses nothing: its `EVAL` faults.
    fn ran(&mut self, code: &'graph BodyShape<'graph>) -> Option<Interval> {
        let unfilled = std::mem::replace(&mut self.unfilled, true);
        let (depth, open) = (self.chain.len(), self.open.len());
        self.push(code, true);
        let level = self.chain.len() - 1;
        self.open_paths(code);
        let walked = self.units_of(level);
        let last = self.last_statement(level);
        self.close_paths(code);
        self.unwind(open);
        self.chain.truncate(depth);
        self.leave(code);
        self.unfilled = unfilled;
        walked.ok().and(last)
    }

    /// Write the closing record `closing` of `segment`, beside the frontier its gathering read.
    fn write_closing(&mut self, segment: Segment, closing: Closing<'o>, frontier: Frontier<'o>) {
        self.store
            .write_close(segment, resident(self.lay, closing), frontier);
    }

    /// Enter the callable `segment` on the demand stack.
    fn enter(&mut self, segment: Segment) {
        let demand = &mut self.demand;
        demand.marks[segment.index() as usize] = Some(Mark {
            index: demand.next,
            low: demand.next,
            on_stack: true,
            self_edge: false,
        });
        demand.stack.push(segment);
        demand.next += 1;
    }

    /// Finish the callable `segment`, its closing computed: written at once where it heads its
    /// component, else kept until its component is finished. A component of more than one body,
    /// or of one that calls itself, is recursive: none of its bodies keeps a summary.
    fn finish(&mut self, segment: Segment, closing: Closing<'o>, frontier: Frontier<'o>) {
        self.demand.pending.push((segment, closing, frontier));
        let mark = self.demand.marks[segment.index() as usize].expect("a callable entered");
        if mark.low != mark.index {
            return;
        }
        let at = (self.demand.stack.iter())
            .rposition(|held| *held == segment)
            .expect("a callable heads its component on the stack");
        let mut members = BumpVec::new_in(self.scratch);
        members.extend(self.demand.stack.drain(at..));
        let recursive = members.len() > 1 || mark.self_edge;
        for member in members.iter() {
            if let Some(mark) = &mut self.demand.marks[member.index() as usize] {
                mark.on_stack = false;
            }
            let held = (self.demand.pending.iter())
                .position(|(pending, _, _)| pending == member)
                .expect("a component's members are closed before it finishes");
            let (_, mut closing, frontier) = self.demand.pending.remove(held);
            closing.recursive = recursive;
            if recursive {
                closing.summary = Some(None);
            }
            self.write_closing(*member, closing, frontier);
        }
    }

    /// The innermost callable the walk is typing, where it is typing one.
    fn caller(&self) -> Option<Segment> {
        self.demand.stack.last().copied()
    }

    /// A kept call from the innermost callable the walk types into `callee`: whether it unfolds,
    /// walking `callee` first where the walk has not reached it. A call into a body still on the
    /// demand stack never unfolds.
    fn kept_call(&mut self, callee: &'graph BodyShape<'graph>) -> Result<bool, ShapeError<'o>> {
        let segment = callee.segment();
        let on_stack = |pass: &Self| {
            pass.demand.marks[segment.index() as usize].is_some_and(|mark| mark.on_stack)
        };
        let reaches = |pass: &mut Self, low: u32| {
            if let Some(caller) = pass.caller()
                && let Some(mark) = &mut pass.demand.marks[caller.index() as usize]
            {
                mark.low = mark.low.min(low);
                if caller == segment {
                    mark.self_edge = true;
                }
            }
        };
        if on_stack(self) {
            let index = self.demand.marks[segment.index() as usize]
                .expect("a callable on the stack is marked")
                .index;
            reaches(self, index);
            return Ok(false);
        }
        if !self.walked[segment.index() as usize] {
            if self.unfilled || !self.typed_on_demand(callee)? {
                return Ok(false);
            }
            if on_stack(self) {
                let low = self.demand.marks[segment.index() as usize]
                    .expect("a callable on the stack is marked")
                    .low;
                reaches(self, low);
                return Ok(false);
            }
        }
        Ok(self
            .closed(segment)
            .is_some_and(|closing| !closing.recursive))
    }

    /// The closing record of `segment` as the walk has it, its frontier gathered: `None` where it
    /// is not written yet.
    fn closed(&self, segment: Segment) -> Option<&'o Closing<'o>> {
        let (closing, frontier) = self.store.close(segment)?;
        self.note(frontier.unknowns());
        Some(closing)
    }

    /// Where the lexical variable at `variable`'s level is read from the shape at chain level
    /// `level`: its home in the shape that declares it, stepped in through every shape between — a
    /// hop per block, a type capture per callable or module, as the shape's layout lays it. The
    /// read is recorded for the contract fold, and each callable or module between composes it.
    fn coordinate_of(
        &mut self,
        level: usize,
        variable: usize,
        (name, used): (TypeSymbol, SourceRef),
    ) -> Coordinate {
        const HOMED: &str = "a lexical variable a static type names has its home on its chain";
        let reading = self.chain[level].shape.segment();
        let coordinate = (self.shapes.coordinate_of(reading, variable)).expect(HOMED);
        if !self.unfilled {
            let tag = self.innermost();
            self.units[tag.0 as usize]
                .named_variables
                .push(NamedVariable {
                    reading,
                    level: variable,
                });
            self.compose(level, variable, (name, used));
        }
        coordinate
    }

    /// Mark the lexical variable at `variable`'s level, named `name` and read for the use at
    /// `used`, composed by each callable or module body between the shape at chain level `level`
    /// and its home. A module body reads a type its enclosing callable binds only where its `OVER`
    /// list names it: it refuses where it is typed.
    fn compose(&mut self, level: usize, variable: usize, (name, used): (TypeSymbol, SourceRef)) {
        let mut at = level;
        loop {
            let shape = self.chain[at].shape;
            if self.homes(shape).iter().any(|(held, _)| *held == variable) {
                return;
            }
            if matches!(shape.kind(), ShapeKind::Callable | ShapeKind::Module) {
                let composed = &mut self.composed[shape.segment().index() as usize];
                if !composed.contains(&variable) {
                    composed.push(variable);
                }
            }
            let name = BinderSymbol::Type(name);
            if shape.kind() == ShapeKind::Module
                && !shape.over().unwrap_or(&[]).contains(&Listed::Name(name))
                && self.unlisted.is_none()
            {
                self.unlisted = Some((shape.segment(), ShapeError::Unlisted { name, at: used }));
            }
            if self.chain[at].roots_chain || at == 0 {
                return;
            }
            at -= 1;
        }
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// What an argument whose static upper end is `upper`, in the shape at `level`, contributes to
    /// a solve where its call runs: `Unknown` — the carried type — where `upper` is `Any`; `upper`
    /// where it is closed; and `upper` beside where each lexical variable it names is read.
    fn contribution(&mut self, level: usize, upper: Parametric, at: SourceRef) -> StaticType<'o> {
        if upper == KType::ANY.into() {
            return Static::Unknown;
        }
        let variables = self.located(level, &[upper], at);
        if variables.is_empty() {
            return self
                .types
                .concrete(upper)
                .map_or(Static::Unknown, Static::Closed);
        }
        Static::Rigid {
            value: upper,
            variables,
        }
    }

    /// The solution `solution` of an instance site in the shape at `level`, as its cell records it:
    /// closed where every entry is concrete, and otherwise beside where the site reads each lexical
    /// variable an entry names.
    fn solution(
        &mut self,
        level: usize,
        solution: &[Parametric],
        at: SourceRef,
    ) -> StaticSolution<'o> {
        let types = self.types;
        let mut closed = BumpVec::with_capacity_in(solution.len(), self.scratch);
        closed.extend(solution.iter().map_while(|each| types.concrete(*each)));
        if closed.len() == solution.len() {
            return StaticSolution::Closed(collect(self.lay, closed.iter().copied()));
        }
        let variables = self.located(level, solution, at);
        assert!(
            !variables.is_empty(),
            "an instance's solution names no variable but its chain's lexical ones"
        );
        StaticSolution::Rigid {
            value: collect(self.lay, solution.iter().copied()),
            variables,
        }
    }

    /// Where the shape at `level` reads each lexical variable `values` name, by level: a hop per
    /// block and a type capture per callable or module between the read and the variable's home,
    /// each added for the use at `used`, and a path variable through the root of a path of its
    /// class. Empty where they name none.
    fn located(&mut self, level: usize, values: &[Parametric], used: SourceRef) -> &'o [Located] {
        let (types, scratch) = (self.types, self.scratch);
        let mut levels: BumpVec<'_, (usize, TypeSymbol)> = BumpVec::new_in(scratch);
        for value in values {
            read_through(types, scratch, *value, Side::Above, &mut |variable| {
                if let Variable::Lexical { level, name, .. } = variable
                    && !levels.iter().any(|(at, _)| *at == level)
                {
                    levels.push((level, name));
                }
                None
            });
        }
        let mut located = BumpVec::with_capacity_in(levels.len(), scratch);
        for (variable, name) in levels.iter().copied() {
            // A path variable is read through a path of its class whose root this shape reaches,
            // the application it is read through located beside it.
            if self.is_path(level, variable) {
                let (entry, at) = self
                    .reachable(level, variable)
                    .expect("a path variable a static type names has a root its shape reaches");
                if let Some(through) = entry.through
                    && self.types.concrete(through).is_none()
                    && !self.has_through(level, entry.level)
                {
                    let through = self.located(level, &[through], used);
                    self.record_through(level, entry.level, through);
                }
                located.push(Located {
                    level: variable,
                    source: Source::Path {
                        at: Some(at),
                        root: self.chain_root_segment(level),
                        level: entry.level,
                    },
                });
                continue;
            }
            let at = self.coordinate_of(level, variable, (name, used));
            located.push(Located {
                level: variable,
                source: Source::Type(at),
            });
        }
        collect(self.lay, located.iter().copied())
    }

    /// A slot's static type before any unit runs: a registration's function type, a type name's
    /// type value's, a parameter's declared type as its body reads it, a name a `USING … SCOPE`
    /// body surfaces from a module typed `surfacing` the member read it names ([`member_read`]),
    /// and `[Never, Any]` for a local its unit sets.
    fn seeded(&self, level: usize, slot: Slot, surfacing: Option<Interval>) -> Bound {
        let shape = self.chain[level].shape;
        // A type name a resumption binds holds the closed type it binds, and a module parameter
        // is exactly the type it binds.
        if let Some(bound) = self.binds(self.unknown_at(level, slot)) {
            return DeclaredType::Type(match shape.slot_name(slot) {
                BinderSymbol::Type(_) => {
                    Interval::point(KType::of_kind(bound.kind_of(self.types)).into())
                }
                _ => Interval::point(bound.into()),
            });
        }
        if let (Some(surfacing), BinderSymbol::Value(_)) = (surfacing, shape.slot_name(slot))
            && shape.position_of(slot) == Position::PARAMETER
        {
            let name = shape.slot_name(slot);
            return match signature_member(self.types, self.scratch, surfacing.upper, name) {
                Some(SignatureMember {
                    declared: DeclaredType::Type(declared),
                    unpinned: None,
                }) => DeclaredType::Type(member_read(self.types, surfacing, declared)),
                Some(SignatureMember {
                    declared: DeclaredType::Scheme(scheme),
                    unpinned: None,
                }) => DeclaredType::Scheme(scheme),
                _ => DeclaredType::Type(unknown()),
            };
        }
        if shape.registration(slot).is_some() {
            return shape
                .births(slot)
                .map_or(DeclaredType::Type(unknown()), |body| {
                    self.callable(level, body)
                });
        }
        let name = shape.slot_name(slot);
        if let BinderSymbol::Type(_) = name {
            return DeclaredType::Type(match self.declared_cell(shape, slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Interval::point(KType::of_kind(handle.kind_of(self.types)).into())
                }
                _ => under(KType::ANY_TYPE.into()),
            });
        }
        let is_parameter = shape.position_of(slot) == Position::PARAMETER;
        if shape.kind() != ShapeKind::Callable || !is_parameter {
            return DeclaredType::Type(unknown());
        }
        let declared = self.in_body(level, Part::Parameter(name.symbol()));
        // Declared where the callable is written, a path there may not be reachable here.
        let typed = retyped_to(self.types, declared.unwrap_or_else(|| KType::ANY.into()));
        DeclaredType::Type(self.sourced(level, typed))
    }

    /// Locate, for the shape at `level`, the lexical variables of each path its `statements`
    /// spell whose declared application names any: where the run reads them to bind the path's
    /// variable. A path whose application names another path is left unlocated, and its type is
    /// read where it runs.
    fn throughs(&mut self, level: usize, spelled: &[u32]) {
        let shape = self.chain[level].shape;
        for index in spelled.iter().map(|at| *at as usize) {
            let Some(path_level) = self.shapes.path_level(shape.segment(), index) else {
                continue;
            };
            let Some(entry) = self.entry_at(level, path_level) else {
                continue;
            };
            let Some(through) = entry.through else {
                continue;
            };
            if self.types.concrete(through).is_some() || self.has_through(level, entry.level) {
                continue;
            }
            let mut paths = false;
            read_through(
                self.types,
                self.scratch,
                through,
                Side::Above,
                &mut |variable| {
                    if let Variable::Lexical { level: at, .. } = variable {
                        paths |= self.is_path(level, at);
                    }
                    None
                },
            );
            if paths {
                continue;
            }
            let spelled = &shape.spelled()[index];
            let site = Site::of(spelled.part);
            let used = source_of(&shape.body()[spelled.statement as usize], site)
                .expect("a path is spelled in its statement");
            let located = self.located(level, &[through], used);
            self.record_through(level, entry.level, located);
        }
    }

    /// Type a component's value binders: each callable it births at its declared type first, then
    /// every other member seeded ([`seed`](Self::seed)) and typed against its fellows' seeds
    /// ([`members`](Self::members)), so no member's type depends on the order the members are
    /// typed in. A component of type binders is the type channel's.
    fn component(&mut self, level: usize, index: usize) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let component = &shape.components()[index];
        if let Some(first) = shape.declarations(component.members[0]) {
            // A nominal declared where it runs, a fresh identity each time its unit does.
            let nominal = (component.members.iter())
                .any(|member| shape.declarations(*member).is_some_and(declares_nominal));
            let closed = (component.members.iter())
                .all(|member| matches!(self.declared_cell(shape, *member), Static::Closed(_)));
            if nominal && !closed {
                self.record_minting(level, Site::of_node(first));
                self.record_nominal(level, Site::of_node(first), component.members);
            }
            return Ok(());
        }
        // A callable member is its declared type, which reads no fellow.
        for member in component.members {
            let Some(body) = shape.births(*member) else {
                continue;
            };
            if body.form().is_none() {
                continue;
            }
            let mut typed = match self.callable(level, body) {
                DeclaredType::Scheme(scheme) => self.bound(level, *member, body, scheme)?,
                typed => typed,
            };
            // The tie births an annotated member at its annotation, so a mention through a
            // fellow's edge reads the binder at its retype, as the slot does.
            if let (DeclaredType::Type(held), Some(annotation)) = (typed, shape.annotation(*member))
            {
                typed = DeclaredType::Type(self.annotated(level, *member, annotation, held)?);
            }
            self.set_binder(level, *member, typed);
        }
        // Every other member is seeded before any is typed: a fellow read while the component is
        // typed is its seed, whatever order the members are typed in.
        for member in component.members {
            if let Some(seed) = self.seed(level, *member) {
                self.set_binder(level, *member, seed);
            }
        }
        self.components.push((level, component.members));
        let typed = self.members(level, component.members);
        self.components.pop();
        for (member, typed) in typed?.iter() {
            self.set_binder(level, *member, DeclaredType::Type(*typed));
        }
        // A binding statement's value is the one binder it writes, where it writes only one.
        let mut values = component
            .members
            .iter()
            .filter(|member| shape.registration(**member).is_none());
        if let (Some(only), None) = (values.next(), values.next()) {
            // A statement binding a quantified callable is typed by no scheme: its value is read
            // only by name, at a call's head.
            if let Some(statement) = self.statement_of(level, *only) {
                let typed = self.binder_fact(level, *only);
                let typed = typed.as_type().unwrap_or_else(unknown);
                self.set_statement(level, statement, typed);
            }
        }
        Ok(())
    }

    /// What a fellow reads `member`, a member of a component of the shape at `level` that births
    /// no callable, as while the component is typed: its annotation, as a retype to it reads, the
    /// signature a `MODULE` or `GROUP` body is at most, or `[Never, Any]`. `None` for a member
    /// that births a callable, which is its declared type already.
    fn seed(&self, level: usize, member: Slot) -> Option<Bound> {
        let shape = self.chain[level].shape;
        let body = shape.births(member);
        if body.is_some_and(|body| body.form().is_some()) {
            return None;
        }
        let module = body.is_some_and(|body| body.kind() == ShapeKind::Module);
        Some(DeclaredType::Type(match shape.annotation(member) {
            Some(annotation) => match self.declared(level, annotation) {
                Some(declared) => retyped_to(self.types, declared),
                None => unknown(),
            },
            None if module => under(KType::EMPTY_SIGNATURE.into()),
            None => unknown(),
        }))
    }

    /// Type `members`, the members of a component of the shape at `level` that birth no callable:
    /// each `MODULE` or `GROUP` body, and each data member from its right-hand side, held to its
    /// annotation. What each is typed as, written once every one is typed.
    fn members(
        &mut self,
        level: usize,
        members: &[Slot],
    ) -> Result<BumpVec<'p, (Slot, Interval)>, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let mut typed = BumpVec::with_capacity_in(members.len(), self.scratch);
        for member in members {
            let annotation = shape.annotation(*member);
            if shape
                .births(*member)
                .is_some_and(|body| body.form().is_some())
                || (shape.births(*member).is_none() && shape.rhs(*member).is_none())
            {
                continue;
            }
            let value = match shape.births(*member) {
                Some(body) if body.form().is_some() => continue,
                Some(body) if body.kind() == ShapeKind::Module => self.module(level, body)?,
                Some(_) => unknown(),
                None => {
                    let Some(rhs) = shape.rhs(*member) else {
                        continue;
                    };
                    let wanted = match annotation {
                        Some(annotation) => self.declared(level, annotation),
                        None => (self.statement_of(level, *member))
                            .and_then(|index| self.tail(level, index)),
                    };
                    self.part_at(level, rhs, wanted)?
                }
            };
            let value = match annotation {
                Some(annotation) => self.annotated(level, *member, annotation, value)?,
                None => value,
            };
            // A module binder a resumption binds is exactly the type it binds, as an annotation
            // retyping it to that type would make it.
            let value = match self.binds(self.unknown_at(level, *member)) {
                Some(bound) => Interval::point(bound.into()),
                None => value,
            };
            self.note_own(level, *member, DeclaredType::Type(value));
            typed.push((*member, value));
        }
        Ok(typed)
    }

    /// The index of the statement that binds `member` in the shape at `level`, where one does.
    fn statement_of(&self, level: usize, member: Slot) -> Option<usize> {
        let shape = self.chain[level].shape;
        shape.position_of(member).statement_index()
    }

    /// The type the statement at `index` of the shape at `level` is wanted at: a callable body's
    /// declared return for its last statement, whose value is the body's.
    fn tail(&self, level: usize, index: usize) -> Option<Parametric> {
        let shape = self.chain[level].shape;
        if shape.kind() != ShapeKind::Callable || index + 1 != shape.body().len() {
            return None;
        }
        let ret = self.in_body(level, Part::Return)?;
        Some(self.sourced(level, under(ret)).upper)
    }

    /// The static type of `member`, a binder of the shape at `level` whose statement births the
    /// quantified function `body` typed by `scheme`. A module body's plain `LET` keeps the scheme —
    /// a member, read at a call's head or instantiated where it is read. Any other `LET` is an
    /// instance site wanted at its annotation, or at the declared return for a callable body's last
    /// statement, and is refused where that fixes nothing. A keyworded form keeps its scheme.
    fn bound(
        &mut self,
        level: usize,
        member: Slot,
        body: &'graph BodyShape<'graph>,
        scheme: Scheme,
    ) -> Result<Bound, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let Some(index) = self.statement_of(level, member) else {
            return Ok(DeclaredType::Scheme(scheme));
        };
        let statement = &shape.body()[index];
        let wanted = match statement.cache().builtin_shape().map(|form| form.id) {
            Some(BuiltinShapeId::LetAnnotated) => shape
                .annotation(member)
                .and_then(|annotation| self.declared(level, annotation)),
            Some(BuiltinShapeId::LetValue) if shape.kind() == ShapeKind::Module => {
                return Ok(DeclaredType::Scheme(scheme));
            }
            Some(BuiltinShapeId::LetValue) => self.tail(level, index),
            _ => return Ok(DeclaredType::Scheme(scheme)),
        };
        let (typed, solution) = self.instantiate(scheme, wanted, statement.source)?;
        if !self.unfilled {
            let solution = self.solution(level, solution, statement.source);
            self.fix_born_instance(body, solution);
        }
        Ok(DeclaredType::Type(typed))
    }

    /// The instance of the quantified function typed by `scheme` at an instance site `at` wanted at
    /// `wanted`: its exact static type beside the solution it is made at. Refused where no function
    /// type is wanted, where the wanted type reaches some variable of the group with nothing, and
    /// where no instance lies under it.
    fn instantiate(
        &self,
        scheme: Scheme,
        wanted: Option<Parametric>,
        at: SourceRef,
    ) -> Result<Made<'p>, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let quantifiers = scheme.group(types).names;
        let Some(wanted) =
            wanted.filter(|wanted| matches!(types.node(*wanted), TypeNode::KFunction { .. }))
        else {
            return Err(self.unfixed(scheme, wanted, at));
        };
        let solution = match instance_under(types, scratch, scheme, wanted) {
            Ok(solution) => solution,
            Err(InstanceFailure::NoInstance) => {
                return Err(ShapeError::NoInstance {
                    scheme,
                    wanted: bound_above(types, scratch, wanted),
                    at,
                });
            }
            Err(InstanceFailure::Unfixed(unfixed)) => {
                return Err(ShapeError::Unfixed {
                    variables: collect(self.lay, unfixed.iter().map(|index| quantifiers[*index])),
                    wanted: Some(bound_above(types, scratch, wanted)),
                    at,
                });
            }
        };
        let solution = solution.leak();
        let instance = instantiate_quantified(types, scratch, scheme, solution);
        Ok((Interval::point(instance), solution))
    }

    /// Where the part at `site` of the shape at `level` is written.
    fn source(&self, level: usize, site: Site) -> SourceRef {
        let statement = self.statement_holding(level, site);
        source_of(&self.chain[level].shape.body()[statement], site)
            .expect("a part lies in its shape's statements")
    }

    /// The statement of the shape at `level` holding the part at `site`: the one the walk is
    /// typing, where it holds the part, else the one a scan of the body finds.
    pub(super) fn statement_holding(&self, level: usize, site: Site) -> usize {
        let at = &self.chain[level];
        let body = at.shape.body();
        if body
            .get(at.statement)
            .is_some_and(|node| part_of(node, site).is_some())
        {
            return at.statement;
        }
        (body.iter().position(|node| part_of(node, site).is_some()))
            .expect("a part lies in its shape's statements")
    }

    /// The instance site `part` of the shape at `level` is, through one-part parentheses: a name
    /// bound to a quantified function, or a quantified `FN` written in place; beside its scheme and
    /// where it is written. `None` for any other part.
    fn instance_site(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Option<(Instanced<'graph>, Scheme, SourceRef)>, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        Ok(match of_part(shape, part) {
            Form::Name(leaf) => {
                let Some(mention) = shape.mention(Site::of(leaf)) else {
                    return Ok(None);
                };
                match self.read_declared(level, mention.coordinate) {
                    DeclaredType::Scheme(scheme) => Some((
                        Instanced::Name(leaf),
                        scheme,
                        self.source(level, Site::of(leaf)),
                    )),
                    DeclaredType::Type(_) => None,
                }
            }
            Form::Lambda(node) => {
                let Some(body) = Site::of_body(node).and_then(|site| shape.nested(site)) else {
                    return Ok(None);
                };
                match self.callable(level, body) {
                    DeclaredType::Scheme(scheme) => {
                        Some((Instanced::Literal(body), scheme, node.source))
                    }
                    DeclaredType::Type(_) => None,
                }
            }
            // One naming a parameter the load cannot name is refused where it is typed.
            Form::Member {
                node,
                operand,
                label,
                ..
            } => match self.peeked_member_scheme(level, operand, label)? {
                Some((scheme, None)) => Some((Instanced::Member(node), scheme, node.source)),
                _ => None,
            },
            _ => None,
        })
    }

    /// [`member_scheme`](Self::member_scheme) where the operand is [peeked](Self::peeked):
    /// `None` for any other.
    fn peeked_member_scheme(
        &mut self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        name: BinderSymbol,
    ) -> Result<Option<(Scheme, Option<BinderSymbol>)>, ShapeError<'o>> {
        match self.peeked(level, operand) {
            Some(peeked) => self.member_scheme(level, operand, peeked.upper, name),
            None => Ok(None),
        }
    }

    /// The member read `ATTR <operand> <label>`, its label written bare, where it reads a quantified
    /// member: the member's scheme, beside the unpinned head parameter it names, if it names one.
    /// `upper` is the operand's static upper end: read without typing it where the operand is a
    /// name or a chain of member reads rooted at one ([`peeked`](Self::peeked)), else once the
    /// read's use has typed it.
    fn member_scheme(
        &mut self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        upper: Parametric,
        name: BinderSymbol,
    ) -> Result<Option<(Scheme, Option<BinderSymbol>)>, ShapeError<'o>> {
        Ok(match self.member_read(level, operand, upper, name)? {
            Some(SignatureMember {
                declared: DeclaredType::Scheme(scheme),
                unpinned,
            }) => Some((scheme, unpinned.map(BinderSymbol::Type))),
            _ => None,
        })
    }

    /// The member read `node` of member `label`, a quantified one whose scheme `member_scheme`
    /// gave: its instance wanted at `wanted`, recorded for the run. Refused where the scheme is over
    /// a head parameter the load cannot name.
    fn member_instance(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        label: BinderSymbol,
        (scheme, unpinned): (Scheme, Option<BinderSymbol>),
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        if let Some(parameter) = unpinned {
            return Err(ShapeError::UnpinnedMember {
                member: label,
                parameter,
                at: node.source,
            });
        }
        let (typed, solution) = self.instantiate(scheme, wanted, node.source)?;
        if !self.unfilled {
            let solution = self.solution(level, solution, node.source);
            self.record_instance_read(level, Site::of_node(node), InstanceRead::Solved(solution));
        }
        Ok(typed)
    }

    /// The static type of `part`, a name or a chain of member reads rooted at one, read without
    /// typing it: a name's binder's, and a member read's as its rule gives it. `None` for any other
    /// part.
    fn peeked(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Interval> {
        let shape = self.chain[level].shape;
        match of_part(shape, part) {
            Form::Name(leaf) => {
                let mention = shape.mention(Site::of(leaf))?;
                self.read_declared(level, mention.coordinate).as_type()
            }
            Form::Member { operand, label, .. } => {
                let operand = self.peeked(level, operand)?;
                match signature_member(self.types, self.scratch, operand.upper, label)? {
                    SignatureMember {
                        declared: DeclaredType::Type(declared),
                        unpinned: None,
                    } => Some(member_read(self.types, operand, declared)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The type a quantified function argument at the slot `slot` of a callee whose group is
    /// bounded by `bounds` is wanted at: the group solved from the `others` first — each other slot
    /// beside the type the solve reads there ([`solved_from`](Self::solved_from)) — then `slot`
    /// read from above through it. A variable an
    /// earlier class than the slot's solves is taken as `fixed` gives it, which is empty where none
    /// is. Any other variable the slot shares with the others must come out one point the call
    /// reproduces — a closed one, or one of a reproducible solve ([`Collector::reproducible`]); one
    /// only the slot names is read through `[Never, bound]`. Refused where the others admit no
    /// solve, or leave a shared variable open.
    fn slot_wanted(
        &self,
        bounds: &[KType],
        slot: Parametric,
        others: &[(Parametric, Interval)],
        fixed: &[Option<Parametric>],
    ) -> Result<Parametric, Unsolved<'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let names = |declared: Parametric, variable: usize| {
            types.references_quantifier(scratch, declared, variable)
        };
        let fixed = |variable: usize| fixed.get(variable).copied().flatten();
        let named = || {
            let mut named = BumpVec::new_in(scratch);
            named.extend((0..bounds.len()).filter(|variable| names(slot, *variable)));
            Unsolved::Open(named)
        };
        let mut collector = Collector::<Parametric>::new(types, scratch, bounds);
        for (variable, bound) in bounds.iter().enumerate() {
            if let Some(to) = fixed(variable) {
                collector.pin(types, variable, *bound, to);
            }
        }
        let mut declared = BumpVec::with_capacity_in(others.len(), scratch);
        let mut exact = true;
        for (other, argument) in others.iter() {
            // One naming only fixed variables is admitted against them by the call, and says
            // nothing of the rest.
            if !(0..bounds.len())
                .any(|variable| names(*other, variable) && fixed(variable).is_none())
            {
                continue;
            }
            let admitted = admits_with(
                types,
                scratch,
                *other,
                argument.upper,
                Variance::Co,
                &mut collector,
            );
            if admitted.is_err() {
                // Only an argument meeting its slot at `Never` never fits; an imprecise one fixes
                // nothing.
                return Err(
                    match never_satisfies(types, scratch, argument.upper, *other) {
                        Some(_) => Unsolved::Misfit,
                        None => named(),
                    },
                );
            }
            exact &= argument.is_exact();
            declared.push(*other);
        }
        let Ok(solution) = collector.solve(types) else {
            return Err(named());
        };
        let reproducible = collector.reproducible(types);
        let solved = intervals(
            types,
            scratch,
            &declared,
            bounds,
            &solution,
            exact && reproducible,
        );
        let mut chosen = BumpVec::with_capacity_in(bounds.len(), scratch);
        let mut open = BumpVec::new_in(scratch);
        for (variable, bound) in bounds.iter().enumerate() {
            if let Some(to) = fixed(variable) {
                chosen.push(Interval::point(to));
                continue;
            }
            // Never `intervals`' answer for a variable no other slot names: it reads as the bound.
            if !declared.iter().any(|other| names(*other, variable)) {
                chosen.push(Interval::within(*bound));
                continue;
            }
            let interval = solved[variable];
            // A concrete point of a solve a run may not reproduce is still the run's: with `exact`
            // false, only a meet of concrete upper contributions comes out one.
            if names(slot, variable)
                && !(interval.is_exact()
                    && (reproducible || types.concrete(interval.upper).is_some()))
            {
                open.push(variable);
            }
            chosen.push(interval);
        }
        if !open.is_empty() {
            return Err(Unsolved::Open(open));
        }
        Ok(read_through(
            types,
            scratch,
            slot,
            Side::Above,
            &mut |variable| match variable {
                Variable::Quantified { index, .. } => chosen.get(index).copied(),
                _ => None,
            },
        ))
    }

    /// The variables of the slot at `position` of the candidate `shape` that a class before that
    /// slot's solves, each solved as the call solves it: class by class, from each earlier class's
    /// solving slots at their contributions the load knows among `contributions`, pinned to what
    /// the classes before it solved. A variable is fixed only where that solve is reproducible
    /// ([`Collector::reproducible`]): the call binds each lexical variable as the load did. `None`
    /// for every other variable, and empty where there is none. `Misfit` where those contributions
    /// never admit at any binding, since the call then never runs; `Open` naming a variable a slot
    /// that solves it reads no contribution at — an instance argument's, or unknown — or one a run
    /// may solve otherwise.
    fn solved_earlier(
        &self,
        shape: DeclaredType<Parametric>,
        position: usize,
        contributions: &[StaticType<'o>],
    ) -> Result<&'p [Option<Parametric>], Unsolved<'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let DeclaredType::Scheme(scheme) = shape else {
            return Ok(&[]);
        };
        let mut slots = BumpVec::new_in(scratch);
        slots.extend(scheme_slots(scheme, types));
        let view = shape.view(types);
        let bounds = view.group.bounds;
        let TypeNode::ExpressionShape { classes, .. } = view.body else {
            unreachable!("a candidate's shape is an expression shape")
        };
        let class = |slot: usize| classes.get(slot);
        let solving = solving_slots(types, scratch, shape);
        let names = |slot: usize, variable: usize| {
            types.references_quantifier(scratch, slots[slot], variable)
        };
        let earlier = |slot: usize| class(slot) < class(position);
        let mut shared = BumpVec::new_in(scratch);
        shared.extend((0..bounds.len()).filter(|variable| {
            names(position, *variable)
                && (0..slots.len()).any(|slot| earlier(slot) && names(slot, *variable))
        }));
        if shared.is_empty() {
            return Ok(&[]);
        }
        let open = |slot: usize| {
            let mut open = BumpVec::new_in(scratch);
            open.extend(shared.iter().copied().filter(|v| names(slot, *v)));
            Unsolved::Open(open)
        };
        let all_open = || {
            let mut open = BumpVec::new_in(scratch);
            open.extend(shared.iter().copied());
            Unsolved::Open(open)
        };
        // Each earlier class in order, in a collector of its own pinned to what the classes before
        // it solved, as `admit_by_class` admits the call. Only the shared variables are pinned: a
        // variable these slots name beside them may take contributions from slots skipped here.
        let first = |variable: usize| {
            (0..slots.len())
                .filter(|slot| names(*slot, variable))
                .map(class)
                .min()
        };
        let mut earlier_classes = BumpVec::new_in(scratch);
        earlier_classes.extend((0..slots.len()).filter(|slot| earlier(*slot)).map(class));
        earlier_classes.sort_unstable();
        earlier_classes.dedup();
        let mut fixed = BumpVec::with_capacity_in(bounds.len(), scratch);
        fixed.resize(bounds.len(), None);
        for current in earlier_classes.iter().copied() {
            let mut collector = Collector::<Parametric>::new(types, scratch, bounds);
            for (variable, to) in fixed.iter().enumerate() {
                if let Some(to) = to {
                    collector.pin(types, variable, bounds[variable], *to);
                }
            }
            let mut admitted_any = false;
            for slot in 0..slots.len() {
                if !(class(slot) == current
                    && solving[slot]
                    && shared.iter().any(|v| names(slot, *v)))
                {
                    continue;
                }
                admitted_any = true;
                let contribution = match contributions[slot] {
                    Static::Closed(contribution) => contribution.into(),
                    Static::Rigid { value, .. } => value,
                    Static::Unknown => return Err(open(slot)),
                };
                let admitted = admits_with(
                    types,
                    scratch,
                    slots[slot],
                    contribution,
                    Variance::Co,
                    &mut collector,
                );
                // A failure a run may not reproduce may admit at some binding: it fixes nothing.
                if admitted.is_err() {
                    return Err(match collector.reproducible(types) {
                        true => Unsolved::Misfit,
                        false => open(slot),
                    });
                }
            }
            if !admitted_any {
                continue;
            }
            let reproducible = collector.reproducible(types);
            let solution = match collector.solve(types) {
                Ok(solution) if reproducible => solution,
                Err(_) if reproducible => return Err(Unsolved::Misfit),
                _ => return Err(all_open()),
            };
            for variable in shared.iter().copied() {
                if first(variable) == Some(current) {
                    fixed[variable] = Some(solution[variable]);
                }
            }
        }
        Ok(scratch.alloc_slice_fill_iter(fixed.iter().copied()))
    }

    /// What `shape`'s solve reads at each slot: a slot that solves, whose argument contributes,
    /// exactly that argument's upper end — the call solves from that type — and every other slot
    /// its argument's static type. `contributions` past its end are `Unknown`.
    fn solved_from(
        &self,
        shape: DeclaredType<Parametric>,
        arguments: &[Interval],
        contributions: &[StaticType<'o>],
    ) -> &'p [Interval] {
        let solving = solving_slots(self.types, self.scratch, shape);
        let mut read = BumpVec::with_capacity_in(arguments.len(), self.scratch);
        read.extend(arguments.iter().enumerate().map(|(position, argument)| {
            let contributed = solving.get(position) == Some(&true)
                && contributions
                    .get(position)
                    .is_some_and(|each| !matches!(each, Static::Unknown));
            match contributed {
                true => Interval::point(argument.upper),
                false => *argument,
            }
        }));
        read.leak()
    }

    /// The instance each of `sites`, the instance arguments of a use whose arguments' static types
    /// are `arguments` and whose contributions are `contributions`, takes at a candidate whose
    /// registered shape is `shape`: wanted at its slot's type, read through the candidate's group
    /// solved from its other arguments where it has one, each as the solve reads it
    /// ([`solved_from`](Self::solved_from)) — a variable a class before the slot's solves taken
    /// from that class's solving slots at their contributions, as the call solves it, class by
    /// class. Refused where the load does not know the shape or makes no instance; `None` where the
    /// other arguments do not fit their slots, which the judge refuses.
    fn instances(
        &self,
        shape: Option<DeclaredType<Parametric>>,
        sites: &[InstanceArgument<'graph>],
        arguments: &[Interval],
        contributions: &[StaticType<'o>],
    ) -> Result<&'p [Made<'p>], Option<ShapeError<'o>>> {
        let (types, scratch) = (self.types, self.scratch);
        let mut slots = BumpVec::new_in(scratch);
        let Some(shape) = shape else {
            return Err(Some(self.unfixed(sites[0].scheme, None, sites[0].at)));
        };
        let (bounds, quantifiers) = match shape {
            DeclaredType::Type(shape) => {
                slots.extend(shape_slots(shape, types));
                (&[][..], &[][..])
            }
            DeclaredType::Scheme(scheme) => {
                slots.extend(scheme_slots(scheme, types));
                let group = scheme.group(types);
                (group.bounds, group.names)
            }
        };
        let read = self.solved_from(shape, arguments, contributions);
        let mut others = BumpVec::with_capacity_in(slots.len(), scratch);
        for (position, slot) in slots.iter().enumerate() {
            if !sites.iter().any(|site| site.position == position) {
                others.push((*slot, read[position]));
            }
        }
        let mut made = BumpVec::with_capacity_in(sites.len(), scratch);
        for site in sites.iter() {
            let slot = slots[site.position];
            let wanted = if bounds.is_empty() {
                slot
            } else {
                let fixed = self.solved_earlier(shape, site.position, contributions);
                match fixed.and_then(|fixed| self.slot_wanted(bounds, slot, &others, fixed)) {
                    Ok(wanted) => wanted,
                    Err(Unsolved::Misfit) => return Err(None),
                    Err(Unsolved::Open(open)) => {
                        return Err(Some(ShapeError::Unfixed {
                            variables: collect(
                                self.lay,
                                open.iter().map(|index| quantifiers[*index]),
                            ),
                            wanted: None,
                            at: site.at,
                        }));
                    }
                }
            };
            made.push(self.instantiate(site.scheme, Some(wanted), site.at)?);
        }
        Ok(made.leak())
    }

    /// `Unfixed` for the quantified function typed by `scheme` at `at`, naming its whole group.
    fn unfixed(&self, scheme: Scheme, wanted: Option<Parametric>, at: SourceRef) -> ShapeError<'o> {
        let (types, scratch) = (self.types, self.scratch);
        ShapeError::Unfixed {
            variables: collect(self.lay, scheme.group(types).names.iter().copied()),
            wanted: wanted.map(|wanted| bound_above(types, scratch, wanted)),
            at,
        }
    }

    /// Refuse a callable body whose static type can never satisfy its declared return — every call
    /// of it would fault on its contract.
    fn returns(&self, level: usize) -> Result<(), ShapeError<'o>> {
        let at = &self.chain[level];
        let shape = at.shape;
        if shape.kind() != ShapeKind::Callable || shape.body().is_empty() {
            return Ok(());
        }
        let Some(ret) = self.in_body(level, Part::Return) else {
            return Ok(());
        };
        let last = shape.body().len() - 1;
        let body = self.statement_at(level, last).upper;
        let (types, scratch) = (self.types, self.scratch);
        // A body that never arrives returns nothing to check.
        if body == KType::NEVER.into() {
            return Ok(());
        }
        if let Some((body, returns)) = never_satisfies(types, scratch, body, ret) {
            return Err(ShapeError::ReturnNeverSatisfied {
                body,
                returns,
                at: shape.body()[last].source,
            });
        }
        Ok(())
    }

    /// The static type of the part `part` of the shape at `level`, wanted at no type.
    fn part(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        self.part_at(level, part, None)
    }

    /// The static type of the part `part` of the shape at `level`, wanted at `wanted`, recorded by
    /// its site.
    fn part_at(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let form = of_part(shape, part);
        let typed = self.form(level, form, wanted)?;
        let typed = self.viewed_tree(level, form, Written::Part(Site::of(part)), typed)?;
        self.record_part(level, Site::of(part), typed);
        Ok(typed)
    }

    /// The static type of the statement `node` of the shape at `level`, wanted at `wanted`.
    fn node(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let form = of_node(shape, node);
        let typed = self.form(level, form, wanted)?;
        self.viewed_tree(level, form, Written::Node(Site::of_node(node)), typed)
    }

    /// `typed`, the static type of a call, ascription or member read at `site` of the shape at
    /// `level` the evaluator reads as `form`, where its tree is a view or a header: exactly the
    /// view's signature over a closed tree and the header's carried type over a closed header, and
    /// at most the view's application otherwise. Any other part's, or one that can never be a
    /// module's, as it is.
    fn viewed_tree(
        &mut self,
        level: usize,
        form: Form<'graph>,
        written: Written,
        typed: Interval,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        if !matches!(
            form,
            Form::Call(..) | Form::Apply(_) | Form::Ascribe(..) | Form::Member { .. }
        ) || bound_above(types, scratch, typed.upper).kind_of(types) != KKind::Signature
        {
            return Ok(typed);
        }
        // The binder whose whole right-hand side the part is keys a leaf its tree falls back to.
        let shape = self.chain[level].shape;
        let named = match written {
            Written::Part(site) => (0..shape.slots() as u32)
                .map(Slot)
                .find(|slot| (shape.rhs(*slot)).is_some_and(|part| Site::of(part) == site)),
            Written::Node(_) => None,
        };
        let tree = self.tree_form(trees::Scope::Chain(level), form, written, named)?;
        // A closed header is exactly the type the load computed for it with the run's own fit.
        if let Tree::Retyped { carried, .. } = tree
            && tree.closed()
        {
            return Ok(Interval::point(carried.into()));
        }
        let Tree::View { application, .. } = tree else {
            return Ok(typed);
        };
        if let Some(viewed) = self.view_of(tree, written.site())? {
            return Ok(Interval::point(viewed.view.into()));
        }
        Ok(match types.concrete(typed.upper) {
            Some(upper) if typed.lower == KType::NEVER.into() && !typed.is_exact() => {
                under(meet(types, scratch, application, upper).into())
            }
            _ => typed,
        })
    }

    /// The static type of a node or part the evaluator reads as `form`, wanted at `wanted`. A
    /// quantified `FN` written here is an instance site, born as the instance `wanted` fixes; at a
    /// call's head it is read by [`head`](Self::head).
    fn form(
        &mut self,
        level: usize,
        form: Form<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        Ok(match form {
            Form::Leaf(part) => self.leaf(level, part, wanted)?,
            Form::Name(part) => self.name(level, part, wanted)?,
            Form::Block(nested) => self.block(level, nested, None)?.unwrap_or_else(unknown),
            Form::Lambda(node) => match Site::of_body(node).and_then(|site| shape.nested(site)) {
                Some(body) => match self.callable(level, body) {
                    DeclaredType::Type(typed) => typed,
                    DeclaredType::Scheme(scheme) => {
                        let (typed, solution) = self.instantiate(scheme, wanted, node.source)?;
                        if !self.unfilled {
                            let solution = self.solution(level, solution, node.source);
                            self.fix_born_instance(body, solution);
                        }
                        typed
                    }
                },
                None => unknown(),
            },
            Form::Declaration => Interval::point(KType::NULL.into()),
            Form::Ascribe(node, mode) => self.ascribe(level, node, mode)?,
            Form::Eval(node) => self.eval(level, node)?,
            Form::Using(node) => self.using(level, node)?,
            Form::Member {
                node,
                list,
                operand,
                label,
            } => {
                let peeked = self.peeked(level, operand);
                let scheme = self.peeked_member_scheme(level, operand, label)?;
                match scheme {
                    Some(scheme) => self.member_instance(level, node, label, scheme, wanted)?,
                    None => {
                        let typed = self.narrow(level, node, list)?;
                        // An operand `peeked` cannot read is typed by the read's use.
                        let site = Site::of(operand);
                        let operand_type = (peeked.is_none())
                            .then(|| self.part_type(level, site))
                            .flatten()
                            .map(|typed| typed.upper);
                        let scheme = match operand_type {
                            Some(upper) => self.member_scheme(level, operand, upper, label)?,
                            None => None,
                        };
                        match scheme {
                            Some(scheme) => {
                                self.member_instance(level, node, label, scheme, wanted)?
                            }
                            None => self.read_through_paths(level, operand, label, typed)?,
                        }
                    }
                }
            }
            Form::Call(node, list) => self.narrow(level, node, list)?,
            Form::Apply(node) => self.apply(level, node)?,
            Form::Unevaluable(_) => unknown(),
        })
    }

    /// The static type of a call's head `head`: a quantified callable's scheme where it reads one —
    /// by name, or a quantified `FN` written in place — and otherwise its part's type, recorded by
    /// its site.
    fn head(
        &mut self,
        level: usize,
        head: &'graph ExpressionPart<'graph>,
    ) -> Result<Bound, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let read = match of_part(shape, head) {
            Form::Name(name) => shape
                .mention(Site::of(name))
                .map(|mention| self.read_declared(level, mention.coordinate)),
            Form::Lambda(node) => Site::of_body(node)
                .and_then(|site| shape.nested(site))
                .map(|body| self.callable(level, body)),
            // A member read at a call's head is read as it is, a quantified member by its scheme:
            // its site is recorded as read as it is, which the run reads as the head's mark.
            Form::Member {
                node,
                list,
                operand,
                label,
            } => {
                if !self.unfilled {
                    self.record_instance_read(level, Site::of_node(node), InstanceRead::AsIs);
                }
                match self.peeked_member_scheme(level, operand, label)? {
                    Some((scheme, None)) => Some(DeclaredType::Scheme(scheme)),
                    // A scheme over a parameter the load cannot name: the run solves the call.
                    Some((_, Some(_))) => {
                        let typed = self.narrow(level, node, list)?;
                        self.record_part(level, Site::of(head), typed);
                        return Ok(DeclaredType::Type(unknown()));
                    }
                    // An operand read only by typing it: the head is read as it is, as `part`
                    // reads a member short of its instance.
                    None if self.peeked(level, operand).is_none() => {
                        let typed = self.narrow(level, node, list)?;
                        let typed = self.read_through_paths(level, operand, label, typed)?;
                        let form = of_part(self.chain[level].shape, head);
                        let typed =
                            self.viewed_tree(level, form, Written::Part(Site::of(head)), typed)?;
                        self.record_part(level, Site::of(head), typed);
                        return Ok(DeclaredType::Type(typed));
                    }
                    None => None,
                }
            }
            _ => None,
        };
        match read {
            Some(DeclaredType::Scheme(scheme)) => Ok(DeclaredType::Scheme(scheme)),
            // A name read at a call's head is called, not passed: escape is the text's.
            _ => self.part(level, head).map(DeclaredType::Type),
        }
    }

    /// `<value> :! <Type>` or `<module> :| <Sig>`: under `:|` a view ([`viewed`](Self::viewed)),
    /// and under `:!` the value held to its type ([`held`](Self::held)), as an annotation holds it.
    fn ascribe(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        mode: Ascription,
    ) -> Result<Interval, ShapeError<'o>> {
        let [operand_part, _, ascribed] = node.parts else {
            unreachable!("an ascription has an operand, its keyword and a type")
        };
        let (types, scratch) = (self.types, self.scratch);
        let declared = self.declared(level, &ascribed.value);
        let typed = self.part_at(level, &operand_part.value, declared)?;
        if mode == Ascription::Opaque {
            // An opaque view hides each unpinned head parameter behind a carrier it mints.
            self.record_minting(level, Site::of_node(node));
            let tree = self.tree(level, &operand_part.value)?;
            let tree = resident(self.lay, tree);
            let site = Site::of_node(node);
            if tree.closed() {
                self.chain[level].closed += 1;
                if let Some(unit) = self.fixing() {
                    unit.closed += 1;
                }
            }
            self.record_view(level, site, tree);
            let signature = declared.and_then(|declared| types.concrete(declared));
            if let Some(signature) = signature {
                let viewed = self.view_tree(signature, *tree, site)?;
                self.record_view_tree(level, site, resident(self.lay, viewed));
            }
            // Over a closed tree the load lays every type the view door computes from types
            // alone, so the run's view interns none of its own.
            if let (Some(bare), Some(signature)) = (tree.bare().digest(), signature) {
                let exact = exact(types, typed);
                // What the operand carries, read off its tree, else by its exact type.
                let seen = self.carried_type(views::Carried::Tree(*tree), site)?;
                debug_assert!(
                    exact.is_none() || seen.is_none() || exact == seen,
                    "a tree reads the type its operand carries"
                );
                let carried = match (seen, exact) {
                    (None, Some(exact)) => views::Carried::Typed(exact),
                    _ => views::Carried::Tree(*tree),
                };
                let held = seen.or(exact);
                if let Some(viewed) = opaque_view_types(held, signature, bare, types, scratch) {
                    self.lay(&viewed, carried, site)?;
                }
            }
            return self.viewed(level, node, typed, declared);
        }
        let held = self.held(level, typed, &ascribed.value, Site::of_node(node));
        held.map_err(|(value, ascribed)| ShapeError::AscriptionNeverSatisfied {
            value,
            ascribed,
            at: node.source,
        })
    }

    /// The view ascription `node` of the shape at `level`, its operand's static type `typed` and
    /// its type `declared` where the load knows it, refused only where every run faults:
    ///
    /// | The load sees | Typed |
    /// |---|---|
    /// | an operand that never arrives | `Never` |
    /// | an operand that can never be a module | refused: no module |
    /// | no type | unknown |
    /// | a closed type the view door never takes | refused: no signature |
    /// | an exact operand that never fits the type | refused |
    /// | otherwise | at most the type |
    ///
    /// Signatures meet to a set, never to `Never`, so only an exact operand can be refused for not
    /// fitting. Settled where the operand's upper end fits the type. A view over a tree the load
    /// sees all of is exact at the view's signature ([`viewed_tree`](Self::viewed_tree)).
    fn viewed(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        typed: Interval,
        declared: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        if typed.upper == KType::NEVER.into() {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        if let Some((value, _)) =
            never_satisfies(types, scratch, typed.upper, KType::EMPTY_SIGNATURE.into())
        {
            return Err(ShapeError::NotAModule {
                value,
                at: node.source,
            });
        }
        let Some(declared) = declared else {
            return Ok(unknown());
        };
        let ascribed = bound_above(types, scratch, declared);
        if types.concrete(declared).is_some() && !view::takes(types, ascribed) {
            return Err(ShapeError::NotASignature {
                ascribed,
                at: node.source,
            });
        }
        let operand = bound_above(types, scratch, typed.upper);
        if operand.kind_of(types) != KKind::Signature || ascribed.kind_of(types) != KKind::Signature
        {
            return Ok(under(declared));
        }
        if fits(types, scratch, typed.upper, declared) {
            self.record_settled(level, Site::of_node(node));
        }
        if let Some(exact) = exact(types, typed)
            && !fits(types, scratch, exact, declared)
        {
            return Err(ShapeError::AscriptionNeverSatisfied {
                value: exact,
                ascribed,
                at: node.source,
            });
        }
        Ok(under(declared))
    }

    /// `LET <name> <type> = <value>`, the binder at `member` of the shape at `level`, whose value
    /// is `typed`: held to `annotation` as `:!` holds its operand, settled by the annotation's site.
    fn annotated(
        &mut self,
        level: usize,
        member: Slot,
        annotation: &'graph ExpressionPart<'graph>,
        typed: Interval,
    ) -> Result<Interval, ShapeError<'o>> {
        let held = self.held(level, typed, annotation, Site::of(annotation));
        held.map_err(|(value, annotated)| {
            let shape = self.chain[level].shape;
            let statement = shape.position_of(member).statement_index();
            ShapeError::AnnotationNeverSatisfied {
                value,
                annotated,
                at: shape.body()[statement.expect("an annotated binder is a statement's")].source,
            }
        })
    }

    /// A value of the static type `typed` held to the type part `part`: that type, exactly where
    /// the retype makes it so ([`retyped_to`]), and settled at `site` where `typed`'s upper end
    /// lies under it. Where the two meet at `Never`, both read through their bounds. A module exact
    /// at a closed type is exactly what the run retypes it to, pinned by the run's own
    /// [`pinned_at_fit`], and refused where it fits a signature type not at all.
    fn held(
        &mut self,
        level: usize,
        typed: Interval,
        part: &'graph ExpressionPart<'graph>,
        site: Site,
    ) -> Result<Interval, (KType, KType)> {
        // A value that never arrives has nothing to check.
        if typed.upper == KType::NEVER.into() {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let Some(declared) = self.declared(level, part) else {
            return Ok(unknown());
        };
        let (types, scratch) = (self.types, self.scratch);
        if let Some(refused) = never_satisfies(types, scratch, typed.upper, declared) {
            return Err(refused);
        }
        if fits(types, scratch, typed.upper, declared) {
            self.record_settled(level, site);
        }
        let exact = exact(types, typed).filter(|exact| exact.kind_of(types) == KKind::Signature);
        if let (Some(exact), Some(closed)) = (exact, types.concrete(declared)) {
            // Signatures meet to a set, never to `Never`, so only an exact module can be refused.
            let signature = matches!(
                types.node(closed),
                TypeNode::Signature { .. }
                    | TypeNode::SignatureApply { .. }
                    | TypeNode::SignatureMeet { .. }
            );
            if signature && pinned_at_fit(types, scratch, exact, closed).is_none() {
                return Err((exact, closed));
            }
            let carried = retype_target(types, scratch, exact.into(), closed)
                .as_type()
                .expect("a module's type is a type");
            self.lay_carried(carried);
            return Ok(Interval::point(carried.into()));
        }
        Ok(retyped_to(types, declared))
    }

    /// `USING <module> SCOPE <body>`: its body's last statement's type, the body typed with each
    /// surfaced name at the member read of the operand it names.
    fn using(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let [_, operand, _, _] = node.parts else {
            unreachable!("a `USING … SCOPE` has its keyword, its module, `SCOPE` and a body")
        };
        let typed = self.part(level, &operand.value)?;
        let shape = self.chain[level].shape;
        let Some(body) = Site::of_body(node).and_then(|site| shape.nested(site)) else {
            return Ok(unknown());
        };
        let tag = self.innermost();
        Ok(self
            .block(level, body, Some((typed, tag)))?
            .unwrap_or_else(unknown))
    }

    /// `EVAL <code> -> <Type>`: its declared type, exactly where the retype makes it so
    /// ([`retyped_to`]), as a registration's call is. An operand that can never be code refuses the
    /// load, and so does traced code whose type meets the declared one at `Never`.
    fn eval(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let [_, operand, _, declared] = node.parts else {
            unreachable!("an `EVAL` has its keyword, its code, `->` and a type")
        };
        let typed = self.part(level, &operand.value)?;
        // An operand that never arrives runs nothing.
        if typed.upper == KType::NEVER.into() {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        // Every `EVAL` enters a frame whose code the load may not see.
        self.exit(level);
        let (types, scratch) = (self.types, self.scratch);
        if let Some((value, _)) =
            never_satisfies(types, scratch, typed.upper, KType::ANY_CODE.into())
        {
            return Err(ShapeError::NotCode {
                value,
                at: node.source,
            });
        }
        let Some(declared) = self.declared(level, &declared.value) else {
            return Ok(unknown());
        };
        // Code that never arrives returns nothing to check.
        if let Some(code) = self.traced(level, &operand.value)
            && code != KType::NEVER
            && let Some((code, returns)) = never_satisfies(types, scratch, code.into(), declared)
        {
            return Err(ShapeError::EvalNeverSatisfied {
                code,
                returns,
                at: node.source,
            });
        }
        Ok(retyped_to(types, declared))
    }

    /// The type the type part `part` of the shape at `level` denotes, where the load knows it.
    fn declared(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Parametric> {
        let typed = self.type_cell(self.chain[level].shape, Site::of(part));
        match self.resolved_type(level, typed) {
            Static::Closed(handle) => Some(handle.into()),
            Static::Rigid { value: handle, .. } => Some(handle),
            Static::Unknown => None,
        }
    }

    /// A leaf part other than a name ([`name`](Self::name)) wanted at `wanted`: a literal's type, a
    /// quote's code type, a type value's kind, or a container's over its parts, end by end. A
    /// container passes `wanted` on to its parts only where it is that container's own type.
    fn leaf(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);
        Ok(match part {
            ExpressionPart::Literal(KLiteral::Number(_)) => Interval::point(KType::NUMBER.into()),
            ExpressionPart::Literal(KLiteral::String(_)) => Interval::point(KType::STR.into()),
            ExpressionPart::Literal(KLiteral::Boolean(_)) => Interval::point(KType::BOOL.into()),
            ExpressionPart::Literal(KLiteral::Null) => Interval::point(KType::NULL.into()),
            ExpressionPart::QuotedExpression(_) => shape
                .nested(Site::of(part))
                .map_or_else(unknown, |code| Interval::point(code.code_type().into())),
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match self.resolved_type(level, self.type_cell(shape, Site::of(part))) {
                    Static::Closed(handle) => {
                        Interval::point(KType::of_kind(handle.kind_of(types)).into())
                    }
                    _ => under(KType::ANY_TYPE.into()),
                }
            }
            ExpressionPart::ListLiteral(items) => {
                let element = wanted.and_then(|wanted| match types.node(wanted) {
                    TypeNode::List { element } => Some(element),
                    _ => None,
                });
                let mut typed = BumpVec::with_capacity_in(items.len(), scratch);
                for item in items.iter() {
                    typed.push(self.part_at(level, item, element)?);
                }
                self.record_composite(
                    level,
                    Site::of(part),
                    items.iter().zip(typed.iter().copied()),
                );
                Interval {
                    lower: list_type(types, scratch, typed.iter().map(|each| each.lower)),
                    upper: list_type(types, scratch, typed.iter().map(|each| each.upper)),
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                let values = wanted.and_then(|wanted| match types.node(wanted) {
                    TypeNode::Dict { value, .. } => Some(value),
                    _ => None,
                });
                let mut typed = BumpVec::with_capacity_in(pairs.len(), scratch);
                for (key, value) in pairs.iter() {
                    let key = self.part(level, key)?;
                    typed.push((key, self.part_at(level, value, values)?));
                }
                let each = pairs.iter().zip(typed.iter());
                self.record_composite(
                    level,
                    Site::of(part),
                    each.flat_map(|((key, value), (k, v))| [(key, *k), (value, *v)]),
                );
                Interval {
                    lower: dict_type(
                        types,
                        scratch,
                        typed.iter().map(|(k, v)| (k.lower, v.lower)),
                    ),
                    upper: dict_type(
                        types,
                        scratch,
                        typed.iter().map(|(k, v)| (k.upper, v.upper)),
                    ),
                }
            }
            ExpressionPart::RecordLiteral(fields) => {
                let declared = wanted.and_then(|wanted| match types.node(wanted) {
                    TypeNode::Record { fields } => Some(fields),
                    _ => None,
                });
                let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
                for (name, value) in fields.iter() {
                    let field = declared.and_then(|declared| declared.get(name.symbol()));
                    typed.push((*name, self.part_at(level, value, field)?));
                }
                let each = fields.iter().zip(typed.iter());
                self.record_composite(
                    level,
                    Site::of(part),
                    each.map(|((_, value), (_, typed))| (value, *typed)),
                );
                Interval {
                    lower: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.lower))),
                    upper: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.upper))),
                }
            }
            ExpressionPart::Keyword(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::MarkedUse(..) => unknown(),
            _ => unreachable!("a name is a `Form::Name`, typed by `name`"),
        })
    }

    /// The static type of the name `part`, wanted at `wanted`: its binder's, and a quantified
    /// function's — an instance site — the instance `wanted` fixes, recorded by its site.
    fn name(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        wanted: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let site = Site::of(part);
        let Some(mention) = self.chain[level].shape.mention(site) else {
            return Ok(unknown());
        };
        Ok(match self.read_declared(level, mention.coordinate) {
            DeclaredType::Type(typed) => typed,
            DeclaredType::Scheme(scheme) => {
                let at = self.source(level, site);
                let (typed, solution) = self.instantiate(scheme, wanted, at)?;
                if !self.unfilled {
                    let solution = self.solution(level, solution, at);
                    self.record_instance_read(level, site, InstanceRead::Solved(solution));
                }
                typed
            }
        })
    }

    /// The static type of what `coordinate`, read in the shape at `level`, holds, a quantified
    /// callable's its scheme. A capture along the chain reads its source's, each path variable
    /// there no path of whose class this shape reaches read through its bounds; one crossing into a
    /// quote's code reads it through its bounds.
    fn read_declared(&self, level: usize, coordinate: Coordinate) -> Bound {
        let Followed {
            landing,
            crossed,
            entered,
        } = self.follow(level, coordinate);
        if let Some(entered) = entered {
            self.note(&[entered]);
        }
        let read = match landing {
            Landing::Builtin(index) => {
                return self
                    .builtins
                    .get(index)
                    .ktype()
                    .map(|ktype| Interval::point(ktype.into()));
            }
            Landing::Slot { level: at, slot } if at < level && !crossed => {
                self.sourced_bound(level, self.binder_at(at, slot))
            }
            Landing::Slot { level, slot } => self.binder_at(level, slot),
            Landing::Open { .. } => return DeclaredType::Type(unknown()),
        };
        if !crossed {
            return read;
        }
        let (types, scratch) = (self.types, self.scratch);
        match read {
            DeclaredType::Type(read) => {
                DeclaredType::Type(under(bound_above(types, scratch, read.upper).into()))
            }
            DeclaredType::Scheme(scheme) => {
                DeclaredType::Scheme(scheme_bound_above(types, scratch, scheme))
            }
        }
    }

    /// Follow `coordinate`, read in the shape at `level`, to where it lands on the chain, by the
    /// shapes' one rule: whether it crossed into a quote's code is whether it took a capture of a
    /// shape rooting a chain through to a shape outside it.
    fn follow(&self, level: usize, coordinate: Coordinate) -> Followed {
        let mut entered = None;
        let landed = follow(self, level, coordinate, |at, capture| {
            if self.chain[at].roots_chain && entered.is_none() {
                let segment = self.chain[at].shape.segment();
                entered = Some((
                    at,
                    Unknown {
                        segment,
                        at: Address::Capture(capture),
                    },
                ));
            }
        });
        // The level a landing ends at a capture of; every capture taken before it was passed.
        let (landing, ends) = match landed {
            Some(Lands::Builtin(index)) => (Landing::Builtin(index), None),
            Some(Lands::Binder { at, slot }) => (Landing::Slot { level: at, slot }, None),
            Some(Lands::Hole { at, .. }) => (Landing::Open { hole: true }, Some(at)),
            Some(Lands::Offered { at, .. }) => (Landing::Open { hole: false }, Some(at)),
            None => (Landing::Open { hole: false }, None),
        };
        Followed {
            landing,
            crossed: entered.is_some_and(|(rooted, _)| ends.is_none_or(|ends| rooted > ends)),
            entered: entered.map(|(_, unknown)| unknown),
        }
    }

    /// The slot `coordinate`, read in the shape at `level`, lands at, whether or not the walk to
    /// it crossed into a quote's code.
    fn landed_slot(&self, level: usize, coordinate: Coordinate) -> Option<(usize, Slot)> {
        match self.follow(level, coordinate).landing {
            Landing::Slot { level, slot } => Some((level, slot)),
            Landing::Builtin(_) | Landing::Open { .. } => None,
        }
    }

    /// `(head argument)`: a construction's identity when the head is a type the load knows, a
    /// call by name's [judged](Self::called) return when the head is a function, else
    /// `[Never, Any]`.
    fn apply(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let [head, argument] = node.parts else {
            unreachable!("an application is a head and its argument")
        };
        let (head, argument) = (&head.value, &argument.value);
        let callee = self.head(level, head)?;
        let (types, scratch) = (self.types, self.scratch);
        if let Some(identity) = self.head_handle(level, head) {
            let payload = self.part(level, argument)?;
            let upper = match construction(types, scratch, identity, payload.upper) {
                Ok(constructed) => constructed,
                Err(ConstructionRefused::NotConstructible(_)) => KType::ANY.into(),
                // A misfit faults; an unsolved family lies under the bare family.
                Err(_) => identity.into(),
            };
            let lower = construction(types, scratch, identity, payload.lower)
                .unwrap_or_else(|_| KType::NEVER.into());
            self.record_composite(
                level,
                Site::of_node(node),
                [(argument, payload)].into_iter(),
            );
            return Ok(Interval { lower, upper });
        }
        match self.callee(callee) {
            Some(callee) => self.called(level, callee, argument, node),
            None => {
                let typed = self.part(level, argument)?;
                if porous_site(self.types, typed)
                    && !self.holds_nothing_minted_part(level, argument, typed)?
                {
                    self.exit(level);
                }
                Ok(unknown())
            }
        }
    }

    /// The function a call by name's head of the static type `head` calls, read as a keyworded
    /// use's lone candidate: its parameters laid out as one class, in symbol order. `None` where
    /// the load knows of no function there.
    fn callee(&self, head: Bound) -> Option<Callee<'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let (function, exact) = match head {
            DeclaredType::Type(interval) => {
                (DeclaredType::Type(interval.upper), interval.is_exact())
            }
            DeclaredType::Scheme(scheme) => (DeclaredType::Scheme(scheme), true),
        };
        let view = function.view(types);
        let TypeNode::KFunction { params, ret } = view.body else {
            return None;
        };
        let mut sorted = BumpVec::with_capacity_in(params.len(), scratch);
        sorted.extend(params.iter());
        sorted.sort_unstable_by_key(|(name, _)| name.symbol());
        let mut elements = BumpVec::with_capacity_in(sorted.len(), scratch);
        elements.extend(
            sorted
                .iter()
                .map(|(_, param)| DispatchTokenElement::Slot(*param)),
        );
        let classes = scratch.alloc_slice_fill_copy(sorted.len(), 0u16);
        // The shape's group may be numbered otherwise than the callee's, and nothing reads one
        // through the other: the shape is judged and returns through its own group, and the frame
        // solves the callee's.
        let shape = types.shape_scheme(
            scratch,
            view.group.names,
            view.group.bounds,
            &elements,
            classes,
            ret,
        );
        let wanted = match function {
            DeclaredType::Type(_) => Some(record_type(types, scratch, params.iter())),
            DeclaredType::Scheme(_) => None,
        };
        Some(Callee {
            function,
            shape: shape.handle,
            names: scratch.alloc_slice_fill_iter(sorted.iter().map(|(name, _)| *name)),
            wanted,
            exact,
        })
    }

    /// The static type `payload` of a call by name's argument laid onto `callee`'s slots: each
    /// slot the field of its name at each end, `Never` below and `Any` above where that end is no
    /// record. Records are width-superset, so every carried record lies above the lower end and
    /// names no field it lacks, and lies under the upper end and names every field it names:
    /// `None` where the lower end is a record lacking a parameter, or the upper end a record naming
    /// a field no parameter declares — the call can never name the parameters exactly.
    fn laid_out(&self, callee: Callee<'_>, payload: Interval) -> Option<&'p [Interval]> {
        let types = self.types;
        let fields = |end: Parametric| match types.node(end) {
            TypeNode::Record { fields } => Some(fields),
            _ => None,
        };
        let (lower, upper) = (fields(payload.lower), fields(payload.upper));
        let declares =
            |field: BinderSymbol| (callee.names.iter()).any(|name| name.symbol() == field.symbol());
        if lower.is_some_and(|lower| {
            (callee.names.iter()).any(|name| lower.get(name.symbol()).is_none())
        }) || upper.is_some_and(|upper| upper.keys().any(|field| !declares(field)))
        {
            return None;
        }
        let mut laid = BumpVec::with_capacity_in(callee.names.len(), self.scratch);
        laid.extend(callee.names.iter().map(|name| {
            Interval {
                lower: (lower.and_then(|lower| lower.get(name.symbol())))
                    .unwrap_or_else(|| KType::NEVER.into()),
                upper: (upper.and_then(|upper| upper.get(name.symbol())))
                    .unwrap_or_else(|| KType::ANY.into()),
            }
        }));
        Some(laid.leak())
    }

    /// A call by name `node` of `callee` over `argument`, judged as a keyworded use of its lone
    /// candidate: the argument [laid out](Self::laid_out) onto the slots, each solving slot's
    /// contribution recorded by the argument's site for the frame, and the shape judged class by
    /// class ([`judge_by_class`]). Refused where the argument can never name the parameters or
    /// fit them; `Never` where a field never arrives. Only an exact callee is judged: one known at
    /// most may bind a function admitting more, or naming other parameters, so its call is at most
    /// its return. A quantified callee's record literal types each field that is an instance site
    /// at its slot read through the group the other fields solve. A porous value at a porous slot
    /// is an exit unless it [holds nothing](Self::holds_nothing_minted) the body minted.
    fn called(
        &mut self,
        level: usize,
        callee: Callee<'p>,
        argument: &'graph ExpressionPart<'graph>,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let never = Interval::point(KType::NEVER.into());
        let at_most = |pass: &Self| Ok(under(pass.returned(callee.shape, None)));
        // The argument's whole static type, where it was typed whole.
        let mut whole = None;
        let (payload, arguments, contributions) = match (callee.function, argument) {
            (DeclaredType::Scheme(_), ExpressionPart::RecordLiteral(fields)) => {
                match self.literal(level, callee, argument, fields, node)? {
                    Some(laid) => laid,
                    None => return Ok(never),
                }
            }
            _ => {
                let payload = self.part_at(level, argument, callee.wanted)?;
                if payload.upper == KType::NEVER.into() {
                    return Ok(never);
                }
                // A callee known at most may bind a function whose parameters admit more.
                let inexact = !callee.exact
                    && porous_site(self.types, payload)
                    && !self.holds_nothing_minted_part(level, argument, payload)?;
                let Some(arguments) = self.laid_out(callee, payload) else {
                    return match callee.exact {
                        true => Err(self.never(callee, payload.upper, node)),
                        false => {
                            if inexact {
                                self.exit(level);
                            }
                            at_most(self)
                        }
                    };
                };
                if arguments
                    .iter()
                    .any(|each| each.upper == KType::NEVER.into())
                {
                    return Ok(never);
                }
                if !callee.exact {
                    if inexact {
                        self.exit(level);
                    }
                    return at_most(self);
                }
                let contributions =
                    self.contributions(level, callee, arguments, node.source, |_| true);
                whole = Some(payload);
                (payload.upper, arguments, contributions)
            }
        };
        if !self.unfilled && (contributions.iter()).any(|each| !matches!(each, Static::Unknown)) {
            let recorded = collect(self.lay, contributions.iter().copied());
            self.record_named(level, Site::of(argument), recorded);
        }
        let (types, scratch) = (self.types, self.scratch);
        let minted_free = self.minted_free_fields(level, callee.names, argument, whole)?;
        if self.slot_exits(callee.shape, arguments, minted_free) {
            self.exit(level);
        }
        let read = self.solved_from(callee.shape, arguments, contributions);
        let judged = judge_by_class(types, scratch, callee.shape, read);
        if judged.verdict == Verdict::Never {
            return Err(self.never(callee, payload, node));
        }
        Ok(self.registered_return(callee.shape, judged.intervals, false))
    }

    /// What each of `callee`'s slots contributes to its solve, the argument there of the static
    /// type in `arguments`: a solving slot `typed` holds its argument's upper end, read as
    /// [`contribution`](Self::contribution) reads it, and every other `Unknown`.
    fn contributions(
        &mut self,
        level: usize,
        callee: Callee<'_>,
        arguments: &[Interval],
        at: SourceRef,
        typed: impl Fn(usize) -> bool,
    ) -> &'o [StaticType<'o>] {
        let solving = solving_slots(self.types, self.scratch, callee.shape);
        let mut contributions = BumpVec::with_capacity_in(arguments.len(), self.scratch);
        for (slot, argument) in arguments.iter().enumerate() {
            contributions.push(match solving.get(slot) == Some(&true) && typed(slot) {
                true => self.contribution(level, argument.upper, at),
                false => Static::Unknown,
            });
        }
        collect(self.lay, contributions.iter().copied())
    }

    /// A quantified callee's record literal `fields`, the argument of the call by name `node`:
    /// each field that is no instance site typed, then each that is at its slot, wanted at the
    /// slot read through the group the others solve ([`instances`](Self::instances)), and the
    /// record recorded by the argument's site. `None` where a field never arrives. Refused where
    /// the fields do not name the parameters exactly, or where the other fields can never fit their
    /// slots.
    fn literal(
        &mut self,
        level: usize,
        callee: Callee<'p>,
        argument: &'graph ExpressionPart<'graph>,
        fields: &'graph [(BinderSymbol, ExpressionPart<'graph>)],
        node: &'graph KExpression<'graph>,
    ) -> Result<Option<Laid<'p, 'o>>, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let slot_of = |name: BinderSymbol| {
            (callee.names.iter()).position(|each| each.symbol() == name.symbol())
        };
        let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
        let mut sites = BumpVec::new_in(scratch);
        for (name, value) in fields.iter() {
            let Some((instanced, scheme, at)) = self.instance_site(level, value)? else {
                typed.push(Some(self.part(level, value)?));
                continue;
            };
            // A field naming no parameter refuses the call below, before it is made.
            if let Some(position) = slot_of(*name) {
                sites.push(InstanceArgument {
                    position,
                    part: value,
                    instanced,
                    scheme,
                    at,
                });
            }
            typed.push(None);
        }
        // Each field's upper end, an untyped instance field's at `Any`: what a refusal names.
        let uppers = |typed: &[Option<Interval>]| {
            let each = fields.iter().zip(typed.iter());
            record_type(
                types,
                scratch,
                each.map(|((name, _), typed)| {
                    (*name, typed.map_or(KType::ANY.into(), |typed| typed.upper))
                }),
            )
        };
        let names_exactly = fields.len() == callee.names.len()
            && fields.iter().all(|(name, _)| slot_of(*name).is_some());
        if !names_exactly {
            return Err(self.never(callee, uppers(&typed), node));
        }
        if typed
            .iter()
            .flatten()
            .any(|typed| typed.upper == KType::NEVER.into())
        {
            let never = Interval::point(KType::NEVER.into());
            self.record_part(level, Site::of(argument), never);
            return Ok(None);
        }
        let mut arguments = BumpVec::with_capacity_in(callee.names.len(), scratch);
        arguments.resize(callee.names.len(), unknown());
        for ((name, _), typed) in fields.iter().zip(typed.iter()) {
            if let (Some(typed), Some(slot)) = (typed, slot_of(*name)) {
                arguments[slot] = *typed;
            }
        }
        let instanced = |slot: usize| sites.iter().any(|site| site.position == slot);
        let contributions = self.contributions(level, callee, &arguments, node.source, |slot| {
            !instanced(slot)
        });
        if !sites.is_empty() {
            let made = match self.instances(Some(callee.shape), &sites, &arguments, contributions) {
                Ok(made) => made,
                Err(None) => return Err(self.never(callee, uppers(&typed), node)),
                Err(Some(error)) => return Err(error),
            };
            for (site, made) in sites.iter().zip(made.iter()) {
                self.record_instance(level, site, *made);
                arguments[site.position] = made.0;
                let field = fields
                    .iter()
                    .position(|(_, value)| std::ptr::eq(value, site.part));
                typed[field.expect("an instance site is a field's value")] = Some(made.0);
            }
        }
        let field = |index: usize| typed[index].expect("every field is typed");
        let ends = |end: fn(Interval) -> Parametric| {
            let each = fields.iter().enumerate();
            record_type(
                types,
                scratch,
                each.map(|(index, (name, _))| (*name, end(field(index)))),
            )
        };
        let payload = Interval {
            lower: ends(|each| each.lower),
            upper: ends(|each| each.upper),
        };
        self.record_part(level, Site::of(argument), payload);
        Ok(Some((payload.upper, arguments.leak(), contributions)))
    }

    /// `CallNeverSatisfied` for the call by name `node` of `callee` over an argument whose static
    /// upper end is `payload`, both read through their bounds.
    fn never(
        &self,
        callee: Callee<'_>,
        payload: Parametric,
        node: &KExpression<'_>,
    ) -> ShapeError<'o> {
        let (types, scratch) = (self.types, self.scratch);
        ShapeError::CallNeverSatisfied {
            callee: match callee.function {
                DeclaredType::Type(function) => {
                    DeclaredType::Type(bound_above(types, scratch, function))
                }
                DeclaredType::Scheme(scheme) => {
                    DeclaredType::Scheme(scheme_bound_above(types, scratch, scheme))
                }
            },
            arguments: bound_above(types, scratch, payload),
            at: node.source,
        }
    }

    /// `ret`, a return under its own group, read through that group's `intervals` — each variable
    /// at `[Never, bound]` where there are none — keeping every lexical variable.
    fn through(&self, ret: Parametric, intervals: Option<&[Interval]>) -> Parametric {
        read_through(
            self.types,
            self.scratch,
            ret,
            Side::Above,
            &mut |variable| match variable {
                Variable::Quantified { index, bound } => Some(
                    intervals
                        .and_then(|all| all.get(index).copied())
                        .unwrap_or(Interval::within(bound)),
                ),
                _ => None,
            },
        )
    }

    /// `shape`'s declared return read through its own group's `intervals`.
    fn returned(
        &self,
        shape: DeclaredType<Parametric>,
        intervals: Option<&[Interval]>,
    ) -> Parametric {
        let ret = match shape {
            DeclaredType::Type(shape) => shape_return(shape, self.types),
            DeclaredType::Scheme(scheme) => scheme_return(scheme, self.types),
        };
        self.through(ret.expect("a registered shape returns"), intervals)
    }

    /// The type an application's head denotes, where the load knows it.
    fn head_handle(&self, level: usize, head: &'graph ExpressionPart<'graph>) -> Option<KType> {
        let shape = self.chain[level].shape;
        match head {
            ExpressionPart::Type(_) => {
                let coordinate = shape.mention(Site::of(head))?.coordinate;
                if let Coordinate::Builtin(index) = coordinate {
                    return match self.builtins.get(index) {
                        Value::Type(value) => Some(value.handle()),
                        _ => None,
                    };
                }
                let (at, slot) = self.landed_slot(level, coordinate)?;
                let holder = self.chain[at].shape;
                holder.declarations(slot)?;
                match self.declared_cell(holder, slot) {
                    Static::Closed(handle) => Some(handle),
                    _ => None,
                }
            }
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match self.type_cell(shape, Site::of(head)) {
                    Static::Closed(handle) => Some(handle),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A keyworded use: its arguments typed, each candidate judged against them — a builtin through
    /// its rule too, over what each slot holds as written — and the use narrowed, or a candidate
    /// selected, where no *maybe* one is left and one ranks first. Its static type is the selected
    /// candidate's [return](Self::candidate_return), or at most the join of what is left's returns.
    fn narrow(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let wanted = slots(node, scratch);
        let mut given = BumpVec::with_capacity_in(wanted.len(), scratch);
        // Each argument that is an instance site is typed per candidate, at the slot it fills.
        let mut sites = BumpVec::new_in(scratch);
        for (position, wanted) in wanted.iter().enumerate() {
            let site = match wanted {
                Wanted::Evaluated(part) => {
                    self.instance_site(level, part)?.map(|site| (part, site))
                }
                Wanted::Label(_) => None,
            };
            if let Some((part, (instanced, scheme, at))) = site {
                sites.push(InstanceArgument {
                    position,
                    part,
                    instanced,
                    scheme,
                    at,
                });
                given.push(Given {
                    typed: unknown(),
                    names: None,
                });
                continue;
            }
            let each = match wanted {
                Wanted::Label(name) => Given {
                    typed: Interval::point(
                        match name {
                            BinderSymbol::Type(_) => KType::TYPE_NAME_TOKEN,
                            _ => KType::IDENTIFIER,
                        }
                        .into(),
                    ),
                    names: Some(std::slice::from_ref(scratch.alloc(*name))),
                },
                Wanted::Evaluated(part) => Given {
                    typed: self.part(level, part)?,
                    names: written_names(part, scratch),
                },
            };
            given.push(each);
        }
        let arguments = typed_of(&given, scratch);
        // Only a porous argument's judgement is read.
        let mut minted_free = BumpVec::with_capacity_in(wanted.len(), scratch);
        for (wanted, typed) in wanted.iter().zip(arguments.iter()) {
            minted_free.push(match wanted {
                Wanted::Evaluated(part) if porous_site(types, *typed) => {
                    self.holds_nothing_minted_part(level, part, *typed)?
                }
                _ => false,
            });
        }
        let minted_free = minted_free.leak();
        let shape = self.chain[level].shape;
        let index = shape
            .candidate_lists()
            .position(Site::of_node(node))
            .expect("a keyworded use's candidate list is recorded at its node");
        // A use one of whose arguments never arrives never runs its call.
        if arguments
            .iter()
            .any(|argument| argument.upper == KType::NEVER.into())
        {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        // What each argument contributes where some candidate's slot solves from it: an instance
        // argument and a label their carried types. No other argument is read, so a type capture
        // is added only where a solve reads it.
        let mut solved = BumpVec::with_capacity_in(wanted.len(), scratch);
        solved.resize(wanted.len(), false);
        for candidate in list.candidates {
            if let Some(registered) = self.candidate(level, *candidate).shape() {
                let solving = solving_slots(types, scratch, registered);
                for (each, solves) in solved.iter_mut().zip(solving) {
                    *each |= *solves;
                }
            }
        }
        let mut contributions = BumpVec::with_capacity_in(wanted.len(), scratch);
        for (position, wanted) in wanted.iter().enumerate() {
            let instance = sites.iter().any(|site| site.position == position);
            contributions.push(match wanted {
                Wanted::Evaluated(_) if solved[position] && !instance => {
                    self.contribution(level, arguments[position].upper, node.source)
                }
                _ => Static::Unknown,
            });
        }
        let contributes = contributions
            .iter()
            .any(|contribution| !matches!(contribution, Static::Unknown));

        // Each argument as a refusal lists it: its upper end, or an instance argument's scheme.
        let lay = self.lay;
        let uppers = || {
            collect(
                lay,
                arguments.iter().enumerate().map(|(position, argument)| {
                    match sites.iter().find(|site| site.position == position) {
                        Some(site) => DeclaredType::Scheme(site.scheme),
                        None => DeclaredType::Type(argument.upper),
                    }
                }),
            )
        };

        let mut judged = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        // The first argument a builtin's need dropped a candidate over, beside what it lacks.
        let mut dropped = None;
        // The last refusal of an instance argument a candidate was dropped over.
        let mut refused = None;
        for candidate in list.candidates {
            // Unfilled code holds no function at an unmarked key's hole.
            if self.unfilled && self.hole(level, *candidate) {
                continue;
            }
            let known = self.candidate(level, *candidate);
            // At this candidate, each instance argument is exactly the instance its slot makes.
            let (mut here, mut given_here) = (arguments, &given[..]);
            let mut instances: &[Made<'_>] = &[];
            if !sites.is_empty() {
                match self.instances(known.shape(), &sites, arguments, &contributions) {
                    Ok(made) => instances = made,
                    Err(error) => {
                        refused = error.or(refused);
                        continue;
                    }
                }
                let mut read = BumpVec::with_capacity_in(given.len(), scratch);
                read.extend(given.iter().copied());
                for (site, (instance, _)) in sites.iter().zip(instances.iter()) {
                    read[site.position].typed = *instance;
                }
                given_here = read.leak();
                here = typed_of(given_here, scratch);
            }
            let (mut verdict, intervals) = match known.shape() {
                Some(registered) => {
                    if contributes {
                        here = self.solved_from(registered, here, &contributions);
                    }
                    let judged = judge_by_class(types, scratch, registered, here);
                    (judged.verdict, judged.intervals)
                }
                None => (Verdict::Maybe, None),
            };
            let builtin = self.builtin(*candidate);
            let mut ruled = None;
            if let Some(builtin) = builtin.filter(|_| verdict != Verdict::Never) {
                let native = Native::of(builtin.id());
                let typed = rules::typed(native, builtin.ktype(), given_here, types, scratch);
                if let Some((slot, lacks)) = typed.dropped {
                    verdict = Verdict::Never;
                    dropped = dropped.or(Some((given_here[slot].typed.lower, lacks)));
                }
                ruled = Some(typed.returns);
            }
            // A surfaced key's list is exactly its head where one head is the whole of the key: a
            // body definition, or a closed signature head, which every function a header or view
            // offers there is retyped to. Otherwise it may hold functions admitting more than the
            // head it is typed by, so it is at most *maybe*.
            let surfaced = self.surfaced(level, *candidate);
            let exact = surfaced.is_some_and(|heads| match heads {
                [SurfacedHead::Body { .. }] => true,
                [SurfacedHead::Signature { .. }] => matches!(known, Known::Closed(_)),
                _ => false,
            });
            if surfaced.is_some() {
                verdict = match verdict {
                    Verdict::Never if exact => Verdict::Never,
                    _ => Verdict::Maybe,
                };
            }
            // A header's or view's frame retypes its value to the closed signature head's return,
            // a scheme's under the selection's own solution.
            let retyped = matches!(surfaced, Some([SurfacedHead::Signature { .. }]))
                && matches!(known, Known::Closed(_));
            if verdict != Verdict::Never {
                judged.push(Judgement {
                    candidate: *candidate,
                    known,
                    surfaced: surfaced.is_some() && !retyped,
                    verdict,
                    intervals,
                    ruled,
                    instances,
                });
            }
        }
        if contributes && !self.unfilled {
            let contributions = collect(self.lay, contributions.iter().copied());
            self.set_contributions(level, index, contributions);
        }
        // The narrowing law's unnarrowed load: see `unnarrowed`.
        if self.unnarrowed {
            self.use_exits(level, arguments, minted_free, &sites, &judged);
            return Ok(unknown());
        }
        if judged.is_empty() {
            // One candidate's own instance refusal, or every candidate's at once.
            if let Some(refused) = refused {
                return Err(match list.candidates {
                    [_] => refused,
                    _ => ShapeError::NoInstanceAtCandidates {
                        key: list.elements,
                        at: node.source,
                    },
                });
            }
            // What the first dropped argument lacks, its lower end named through its bounds.
            let missing = dropped.and_then(|(lower, lacks)| {
                let (of, at) = (bound_above(types, scratch, lower), node.source);
                Some(match lacks? {
                    Lacks::Field(field) => ShapeError::NoField { of, field, at },
                    Lacks::Member(member) => ShapeError::NoMember { of, member, at },
                })
            });
            return Err(missing.unwrap_or_else(|| ShapeError::NoAdmittingCandidate {
                key: list.elements,
                arguments: uppers(),
                at: node.source,
            }));
        }
        // A *maybe* an *always* one strictly outranks at the first class never runs.
        let always = || {
            judged.iter().filter_map(|other| match other.known {
                Known::Closed(always) if other.verdict == Verdict::Always => Some(always.into()),
                _ => None,
            })
        };
        let mut left = BumpVec::with_capacity_in(judged.len(), scratch);
        left.extend(judged.iter().copied().filter(|judgement| {
            !matches!(judgement.known, Known::Closed(maybe)
                if judgement.verdict == Verdict::Maybe
                    && select::never_runs(types, scratch, maybe.into(), always()))
        }));
        let judged = left;

        if judged
            .iter()
            .all(|judgement| judgement.verdict == Verdict::Always)
        {
            let chosen = match judged[..] {
                [only] => Some(only),
                _ if judged
                    .iter()
                    .all(|judgement| matches!(judgement.known, Known::Closed(_))) =>
                {
                    let mut shapes: BumpVec<'_, DeclaredType<Parametric>> =
                        BumpVec::with_capacity_in(judged.len(), scratch);
                    shapes.extend(judged.iter().map(|judgement| match judgement.known {
                        Known::Closed(registered) => DeclaredType::<Parametric>::from(registered),
                        _ => unreachable!("every candidate is closed"),
                    }));
                    let survivors = select_by_class(types, scratch, &shapes);
                    let builtin =
                        |survivor: usize| self.builtin(judged[survivor].candidate).is_some();
                    match select::winner(&survivors, builtin) {
                        Ok(survivor) => Some(judged[survivor]),
                        Err(count) => {
                            return Err(ShapeError::Ambiguous {
                                key: list.elements,
                                arguments: uppers(),
                                count,
                                at: node.source,
                            });
                        }
                    }
                }
                // Several, a rigid one among them: ranked at the call.
                _ => None,
            };
            if let Some(chosen) = chosen {
                let Candidate::One(coordinate) = chosen.candidate else {
                    unreachable!("an always candidate is no spread");
                };
                self.use_exits(level, arguments, minted_free, &sites, &[chosen]);
                self.made(level, node, list, &sites, &[chosen])?;
                if !self.unfilled {
                    let mut points = BumpVec::new_in(scratch);
                    points.extend(
                        (chosen.intervals.unwrap_or(&[]).iter())
                            .map(|interval| exact(types, *interval)),
                    );
                    let points = collect(self.lay, points.iter().copied());
                    self.record_selection(level, Site::of_node(node), points);
                }
                self.set_narrowing(level, index, Narrowing::Selected(coordinate));
                return Ok(self.candidate_return(chosen));
            }
        }
        if judged.len() < list.candidates.len()
            || judged
                .iter()
                .any(|judgement| judgement.verdict == Verdict::Always)
        {
            let kept = collect(
                self.lay,
                judged
                    .iter()
                    .map(|judgement| (judgement.candidate, judgement.verdict)),
            );
            self.set_narrowing(level, index, Narrowing::Kept(kept));
        }
        self.use_exits(level, arguments, minted_free, &sites, &judged);
        self.made(level, node, list, &sites, &judged)?;
        // A lone closed signature head, with no hole skipped, is the only list that may admit, and
        // every function in it returns retyped to the head's return.
        if let [only] = judged[..]
            && !self.unfilled
            && !only.surfaced
            && self.surfaced(level, only.candidate).is_some()
        {
            return Ok(self.candidate_return(only));
        }
        let mut returns = BumpVec::with_capacity_in(judged.len(), scratch);
        for judgement in judged.iter() {
            if let Known::Unknown = judgement.known {
                return Ok(unknown());
            }
            returns.push(self.candidate_return(*judgement).upper);
        }
        Ok(under(types.union_of(scratch, &returns)))
    }

    /// Mark the shape at `level` porous where the keyworded use whose arguments are typed
    /// `arguments` hands a porous value to a porous slot of a frame it may enter: at a kept
    /// candidate that is not a builtin — a builtin's native runs in the caller's step — whose slot
    /// at that argument is porous, or which the load cannot read: an unknown one, a spread, a hole
    /// or a surfaced key. Each instance argument is read at the instance its candidate makes. An
    /// argument `minted_free` marks as [holding nothing minted](Self::holds_nothing_minted) is no
    /// exit.
    fn use_exits(
        &mut self,
        level: usize,
        arguments: &[Interval],
        minted_free: &[bool],
        sites: &[InstanceArgument<'graph>],
        kept: &[Judgement<'_>],
    ) {
        let scratch = self.scratch;
        let exits = kept.iter().any(|judgement| {
            if self.builtin(judgement.candidate).is_some() {
                return false;
            }
            let mut here = BumpVec::with_capacity_in(arguments.len(), scratch);
            here.extend(arguments.iter().copied());
            for (site, (instance, _)) in sites.iter().zip(judgement.instances) {
                here[site.position] = *instance;
            }
            match (judgement.candidate, judgement.known.shape()) {
                (Candidate::One(_), Some(shape)) => self.slot_exits(shape, &here, minted_free),
                _ => (here.iter().zip(minted_free))
                    .any(|(typed, free)| !free && porous_site(self.types, *typed)),
            }
        });
        if exits {
            self.exit(level);
        }
    }

    /// Whether some argument of the static types `arguments`, not one `minted_free` marks as holding
    /// nothing the frame body minted, is porous where `shape`'s slot at it is too.
    fn slot_exits(
        &self,
        shape: DeclaredType<Parametric>,
        arguments: &[Interval],
        minted_free: &[bool],
    ) -> bool {
        let types = self.types;
        let mut slots = BumpVec::new_in(self.scratch);
        match shape {
            DeclaredType::Type(shape) => slots.extend(shape_slots(shape, types)),
            DeclaredType::Scheme(scheme) => slots.extend(scheme_slots(scheme, types)),
        }
        (slots.iter().zip(arguments).zip(minted_free)).any(|((slot, typed), free)| {
            !free && porous_site(types, *typed) && porous_slot(types, *slot)
        })
    }

    /// Whether `part` is a value the frame body at `level` received — a parameter of that body, a
    /// capture, a hole or a name offered, or a `LET` bound bare to one, named bare, or a member read
    /// of one through the builtin `ATTR` alone — which cannot hold what the body mints, so passing
    /// it on is no exit
    /// ([type tiers](../../design/type-tiers.md#two-classes-of-body)).
    fn received(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> bool {
        self.received_form(level, of_part(self.chain[level].shape, part))
    }

    /// [`received`](Self::received) over what the evaluator reads as `form`.
    fn received_form(&self, level: usize, form: Form<'graph>) -> bool {
        let shape = self.chain[level].shape;
        let name = match form {
            Form::Name(name) => name,
            // A registration at `ATTR`'s key may hand back anything its body reaches.
            Form::Member { list, operand, .. } => {
                return (list.candidates.iter()).all(|each| self.builtin(*each).is_some())
                    && self.received(level, operand);
            }
            _ => return false,
        };
        let Some(mention) = shape.mention(Site::of(name)) else {
            return false;
        };
        let frame = self.frame_level(level);
        match self.follow(level, mention.coordinate).landing {
            Landing::Slot { level: at, slot } => self.received_binder(at, slot, frame),
            Landing::Open { .. } | Landing::Builtin(_) => true,
        }
    }

    /// Whether the binder at `slot` of the shape at `at` holds a value the frame body at `frame`
    /// received: a binder of a body around the frame body, a parameter of the frame body, or a
    /// `LET` bound bare to such a value, which holds that very value. A fellow read is the cyclic
    /// rule's, and is no such value.
    fn received_binder(&self, at: usize, slot: Slot, frame: usize) -> bool {
        let shape = self.chain[at].shape;
        if at < frame || (at == frame && shape.position_of(slot) == Position::PARAMETER) {
            return true;
        }
        shape.rhs(slot).is_some_and(|rhs| {
            rhs.is_name()
                && shape
                    .mention(Site::of(rhs))
                    .is_some_and(|mention| !mention.fellow)
                && self.received(at, rhs)
        })
    }

    /// The static type the pass recorded for `part` of the shape at `level`, where it typed it.
    fn typed_part(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Interval> {
        self.part_type(level, Site::of(part))
    }

    /// Whether the value `form`, written as `written` in the shape at `level` and of the static
    /// type `typed`, holds nothing its frame body minted: a value that is not
    /// [porous](self::porous_site), a value it [received](Self::received_form), a record literal
    /// written in place of such values, or a module whose content tree
    /// [reaches](Self::reaches_minted) nothing minted
    /// ([type tiers](../../design/type-tiers.md#two-classes-of-body)). A record bound by `LET` is
    /// read by its static type alone.
    fn holds_nothing_minted(
        &mut self,
        level: usize,
        form: Form<'graph>,
        written: Written,
        typed: Interval,
    ) -> Result<bool, ShapeError<'o>> {
        if !porous_site(self.types, typed) || self.received_form(level, form) {
            return Ok(true);
        }
        if let Form::Leaf(ExpressionPart::RecordLiteral(fields)) = form {
            for (_, value) in fields.iter() {
                let Some(typed) = self.typed_part(level, value) else {
                    return Ok(false);
                };
                if !self.holds_nothing_minted_part(level, value, typed)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let (types, scratch) = (self.types, self.scratch);
        if bound_above(types, scratch, typed.upper).kind_of(types) != KKind::Signature {
            return Ok(false);
        }
        let tree = self.tree_form(trees::Scope::Chain(level), form, written, None)?;
        Ok(!self.reaches_minted(level, &tree))
    }

    /// [`holds_nothing_minted`](Self::holds_nothing_minted) over the part `part`.
    fn holds_nothing_minted_part(
        &mut self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        typed: Interval,
    ) -> Result<bool, ShapeError<'o>> {
        let form = of_part(self.chain[level].shape, part);
        self.holds_nothing_minted(level, form, Written::Part(Site::of(part)), typed)
    }

    /// Whether the content tree `tree`, read in the shape at `level`, reaches content its frame
    /// body minted: a view, open or closed, since a closed one may still mint past the tree's
    /// horizon; a closure, whose type or captures may name it; a module body holding a minting
    /// site, in itself or in a block or module body it runs, or capturing what reaches it; or a
    /// leaf on anything but a value the frame body received. A leaf reading a member, or keyed on
    /// an unfolded call, reaches it too: the tree does not say the member read was the builtin's.
    fn reaches_minted(&self, level: usize, tree: &Tree<'o>) -> bool {
        let frame = self.frame_level(level);
        let mut stack = BumpVec::new_in(self.scratch);
        stack.push(*tree);
        while let Some(tree) = stack.pop() {
            match tree {
                Tree::Digest(_) => {}
                Tree::Retyped { source, .. } => stack.push(*source),
                Tree::Body { body, captures, .. } => {
                    let shape = self.shape_at(body);
                    let mints = self.module_mints.get(&address(shape)).copied();
                    // A read of the program's top level is no capture the tree holds; it is the
                    // program's own where the program is the frame body.
                    let reads_top = frame == 0
                        && (0..self.capture_count(shape))
                            .any(|at| shape.top_capture(CaptureSlot(at as u32)).is_some());
                    if mints != Some(false) || reads_top {
                        return true;
                    }
                    stack.extend(captures.iter().map(|(_, at)| *at));
                }
                Tree::View { .. } | Tree::Knot { .. } | Tree::Leaf(Leaf::Unnamed(_)) => {
                    return true;
                }
                Tree::Leaf(Leaf::Bound {
                    binder,
                    chain,
                    within,
                    ..
                }) => {
                    if !chain.is_empty() || matches!(within, Some(Within::Call(_))) {
                        return true;
                    }
                    let at = (0..=level)
                        .rev()
                        .find(|at| self.chain[*at].shape.segment() == binder.body);
                    if !at.is_some_and(|at| self.received_binder(at, binder.slot, frame)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// For each of `names`, the slots of a call by name, whether `argument` passes it a value that
    /// [holds nothing](Self::holds_nothing_minted) the frame body at `level` minted: a record
    /// literal field by field, any other argument whole, of the static type `whole`.
    fn minted_free_fields(
        &mut self,
        level: usize,
        names: &[BinderSymbol],
        argument: &'graph ExpressionPart<'graph>,
        whole: Option<Interval>,
    ) -> Result<&'p [bool], ShapeError<'o>> {
        let scratch = self.scratch;
        let mut free = BumpVec::with_capacity_in(names.len(), scratch);
        match argument {
            ExpressionPart::RecordLiteral(fields) => {
                for name in names {
                    let value = (fields.iter())
                        .find(|(field, _)| field.symbol() == name.symbol())
                        .map(|(_, value)| value);
                    let typed =
                        value.and_then(|value| Some((value, self.typed_part(level, value)?)));
                    free.push(match typed {
                        Some((value, typed)) => {
                            self.holds_nothing_minted_part(level, value, typed)?
                        }
                        None => false,
                    });
                }
            }
            _ => {
                let free_whole = match whole {
                    Some(typed) => self.holds_nothing_minted_part(level, argument, typed)?,
                    None => false,
                };
                free.resize(names.len(), free_whole);
            }
        }
        Ok(free.leak())
    }

    /// Record the instance each of `sites`, the instance arguments of the keyworded use `node`,
    /// takes at the candidates `kept`, which must agree on it.
    fn made(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
        sites: &[InstanceArgument<'graph>],
        kept: &[Judgement<'_>],
    ) -> Result<(), ShapeError<'o>> {
        for (index, site) in sites.iter().enumerate() {
            let made = kept[0].instances[index];
            if kept
                .iter()
                .any(|judgement| judgement.instances[index].1 != made.1)
            {
                return Err(ShapeError::AmbiguousInstance {
                    key: list.elements,
                    at: node.source,
                });
            }
            self.record_instance(level, site, made);
        }
        Ok(())
    }

    /// Record the instance `made` the instance argument `site` of the shape at `level` takes: its
    /// part's static type, and its solution by its site or in its body's born-instance cell.
    fn record_instance(&mut self, level: usize, site: &InstanceArgument<'graph>, made: Made<'_>) {
        let (typed, solution) = made;
        self.record_part(level, Site::of(site.part), typed);
        if self.unfilled {
            return;
        }
        let solution = self.solution(level, solution, site.at);
        let read = InstanceRead::Solved(solution);
        match site.instanced {
            Instanced::Name(leaf) => self.record_instance_read(level, Site::of(leaf), read),
            Instanced::Literal(body) => self.fix_born_instance(body, solution),
            Instanced::Member(node) => self.record_instance_read(level, Site::of_node(node), read),
        }
    }

    /// What a judged candidate returns: a builtin's return as its [rule](super::rules) gives it,
    /// and a registration's its [registered return](Self::registered_return).
    fn candidate_return(&self, judgement: Judgement<'_>) -> Interval {
        if let Some(ruled) = judgement.ruled {
            return ruled;
        }
        let Some(registered) = judgement.known.shape() else {
            return unknown();
        };
        self.registered_return(registered, judgement.intervals, judgement.surfaced)
    }

    /// What a call of the registered shape `registered` returns, its group solved to `intervals`:
    /// its return read through them — exactly that return where the retype makes it so and the
    /// solve is the call's, the shape unquantified or every interval a point, since its frame
    /// retypes its value to it, and at most it otherwise. A `surfaced` head's return is at most
    /// whatever the solve: the module's own definition answers the call, and the head states only
    /// a bound on what it returns — save a closed signature head, which the call's frame retypes
    /// to under the selection's solution, and which the caller passes as not `surfaced`.
    fn registered_return(
        &self,
        registered: DeclaredType<Parametric>,
        intervals: Option<&[Interval]>,
        surfaced: bool,
    ) -> Interval {
        let returned = self.returned(registered, intervals);
        let solved = !surfaced
            && match registered {
                DeclaredType::Type(_) => true,
                DeclaredType::Scheme(_) => {
                    intervals.is_some_and(|all| all.iter().all(|each| each.is_exact()))
                }
            };
        if solved {
            retyped_to(self.types, returned)
        } else {
            under(returned)
        }
    }

    /// The type of the code the part `operand` runs, where the load traces it to a written quote:
    /// its last statement's upper end as it runs unfilled, read through its bounds — traced code is
    /// written, so nothing composed or `USING` filled it.
    fn traced(&self, level: usize, operand: &'graph ExpressionPart<'graph>) -> Option<KType> {
        let code = self.traced_code(level, operand)?;
        let last = self.ran_of(code)?;
        Some(bound_above(self.types, self.scratch, last.upper))
    }

    /// The code shape the part `operand` runs, where the load traces it to a written quote: the
    /// quote itself, or a name a `LET` binds to one. Code names extend this; composed code has no
    /// shape at load.
    fn traced_code(
        &self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
    ) -> Option<&'graph BodyShape<'graph>> {
        let shape = self.chain[level].shape;
        match operand {
            ExpressionPart::QuotedExpression(_) => shape.nested(Site::of(operand)),
            ExpressionPart::Identifier(_) => {
                let coordinate = shape.mention(Site::of(operand))?.coordinate;
                let (at, slot) = self.landed_slot(level, coordinate)?;
                let holder = self.chain[at].shape;
                match holder.rhs(slot)? {
                    rhs @ ExpressionPart::QuotedExpression(_) => holder.nested(Site::of(rhs)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether `candidate`, read in the shape at `level`, is an unmarked key's hole: a spread a
    /// `USING` fills. A hole past a quote's code root is not this code's.
    fn hole(&self, level: usize, candidate: Candidate) -> bool {
        let Candidate::Spread(coordinate) = candidate else {
            return false;
        };
        matches!(
            self.follow(level, coordinate),
            Followed {
                landing: Landing::Open { hole: true },
                crossed: false,
                ..
            }
        )
    }

    /// The builtin `candidate` names, where it names one.
    fn builtin(&self, candidate: Candidate) -> Option<&'graph BuiltinFunction> {
        match candidate {
            Candidate::One(Coordinate::Builtin(index)) => self
                .builtins
                .get(index)
                .as_callable()
                .and_then(|member| member.builtin()),
            _ => None,
        }
    }

    /// The heads a `USING … SCOPE` block's surfaced key holds, where `candidate`, read from the
    /// shape at `level`, is one.
    fn surfaced(
        &self,
        level: usize,
        candidate: Candidate,
    ) -> Option<&'graph [SurfacedHead<'graph>]> {
        let Candidate::Spread(coordinate) = candidate else {
            return None;
        };
        let (at, slot) = self.landed_slot(level, coordinate)?;
        self.chain[at].shape.registration(slot)?.surfaced
    }

    /// What the load knows of `candidate`'s registered shape, read from the shape at `level`. A
    /// candidate reached across a quote's code root whose registered shape names a variable of the
    /// chain outside is unknown: the code roots a chain of its own.
    fn candidate(&self, level: usize, candidate: Candidate) -> Known {
        let coordinate = match candidate {
            // A surfaced key's list is typed by its head; any other spread is unknown.
            Candidate::Spread(coordinate) if self.surfaced(level, candidate).is_some() => {
                coordinate
            }
            Candidate::Spread(_) => return Known::Unknown,
            Candidate::One(Coordinate::Builtin(_)) => {
                return match self.builtin(candidate) {
                    Some(builtin) => Known::Closed(builtin.ktype().into()),
                    None => Known::Unknown,
                };
            }
            Candidate::One(coordinate) => coordinate,
        };
        let followed = self.follow(level, coordinate);
        if let Some(entered) = followed.entered {
            self.note(&[entered]);
        }
        let Followed {
            landing: Landing::Slot { level: at, slot },
            crossed,
            ..
        } = followed
        else {
            return Known::Unknown;
        };
        let holder = self.chain[at].shape;
        if holder.registration(slot).is_none() {
            return Known::Unknown;
        }
        let typed = self.registered_cell(holder, slot);
        // One reading a path no path of whose class this shape reaches is unknown here.
        if let Static::Rigid { variables, .. } = typed
            && !self.reaches(level, variables)
        {
            return Known::Unknown;
        }
        match self.resolved_registered(at, typed) {
            Static::Closed(registered) => Known::Closed(registered.shape),
            Static::Rigid { value, .. } if !crossed => Known::Rigid(value.shape),
            Static::Rigid { .. } | Static::Unknown => Known::Unknown,
        }
    }
}

/// The names the part `part` holds as written: a one-name quote's name, or a list literal of
/// one-name quotes' names. `None` for anything else, which the load cannot read.
fn written_names<'x>(
    part: &ExpressionPart<'_>,
    scratch: BumpAllocator<'x>,
) -> Option<&'x [BinderSymbol]> {
    let quoted = |part: &ExpressionPart<'_>| match part {
        ExpressionPart::QuotedExpression(node) => one_name(node.reference()),
        _ => None,
    };
    let mut names = BumpVec::new_in(scratch);
    match part {
        ExpressionPart::ListLiteral(items) => {
            for item in items.iter() {
                names.push(quoted(item)?);
            }
        }
        part => names.push(quoted(part)?),
    }
    Some(names.leak())
}

/// Each argument's static type, read off what a call is given.
fn typed_of<'p>(given: &[Given<'_>], scratch: BumpAllocator<'p>) -> &'p [Interval] {
    let mut typed = BumpVec::with_capacity_in(given.len(), scratch);
    typed.extend(given.iter().map(|each| each.typed));
    typed.leak()
}

/// A shape's address, which keys the pass's tables of what it read off each shape.
fn address(shape: &BodyShape<'_>) -> usize {
    std::ptr::from_ref(shape).addr()
}

#[cfg(test)]
thread_local! {
    /// Whether the walk leaves every keyworded use whole and refuses none: what the narrowing law
    /// loads a program under to see the call a narrowing or a refusal stands for.
    static UNNARROWED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `load` with every keyworded use left whole — no narrowing, no refusal — and each argument's
/// contribution still recorded.
#[cfg(test)]
pub(super) fn unnarrowed<R>(load: impl FnOnce() -> R) -> R {
    /// Clears the switch however `load` ends, so a panicking case leaves it off for the next.
    struct Cleared;
    impl Drop for Cleared {
        fn drop(&mut self) {
            UNNARROWED.with(|switch| switch.set(false));
        }
    }
    UNNARROWED.with(|switch| switch.set(true));
    let _cleared = Cleared;
    load()
}

/// Whether loads leave every keyworded use whole now.
fn unnarrowed_now() -> bool {
    #[cfg(test)]
    return UNNARROWED.with(std::cell::Cell::get);
    #[cfg(not(test))]
    false
}
