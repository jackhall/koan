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
//! variables of its **chain** — the shapes from the program or a composed code down, a written
//! quote's code on its encloser's chain, at the levels the shape numbers them at. A capture is
//! followed to its source along the chain by the rule the shape builder and the shape table follow
//! it by ([`follow`]), which reports a crossing into code: a read crossing through a module
//! binder's capture is written over that capture, and any other reads as its source does, so a
//! candidate it reaches across the code's root is rigid over the chain's names.
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
//! the certain ambiguity. Where every candidate kept is one closed function, the load records how
//! they rank, a table of each pair's answer at each class, so a call ranks nothing; a rigid one is
//! ranked where a settlement or the call closes it. A builtin candidate is judged through its native's [type rule](super::rules)
//! too: an argument whose lower end lies outside the type the rule needs drops it, and a use whose
//! last candidate a need dropped is refused in the native's words. A builtin's call is the return
//! its rule gives — `FROM`'s the projection of its record and `ATTR`'s the named field's type. An
//! `EVAL` is its declared type, as a registration's call is, and code the load traces to a written
//! quote is checked against it as a body is checked against its return. A
//! quantified candidate's return is read through its group's intervals. A use's type is the union
//! of each candidate's return on what it does at the call — never runs, may run, or is selected —
//! as every binding agrees ([`scope::runs`](crate::scope::runs)): closed where they do, else a
//! decided name per candidate on that question, so it hands back the same type at load and under
//! every binding, whatever a witness decides of the selection. A call whose callee and
//! solve the load knows exactly — a selected registration, or a call by name of an exact callee —
//! is exactly the return its frame retypes to; a call in tail position never finishes, and is
//! typed as any other. A callable body's tail call is recorded with its **route**: per candidate,
//! whether its return keeps the frame's contract where both are closed, and whether it is a self
//! tail edge of a body whose name does not escape. A callable body whose static type meets its declared return at `Never`
//! refuses the load too, as does an ascription whose operand's static type meets its type at
//! `Never`; one whose operand's static upper end lies under its type is **settled**, and the run
//! checks nothing. An annotated binder, `LET <name> <type> = <value>`, is held to its type as an
//! ascription holds its operand, settled by its type part's site. Where the type is closed, the
//! walk records what the retype re-stamps at ([`Retype`]) by that site, as it does for a module
//! member read over an exact module and, in a callable's closing, for each parameter and its
//! return: the run re-stamps by the record and computes no target.
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
//! Inside a quote's code a refusal, and the first registration overlapping a builtin, is kept in
//! the code's closing record, and the `EVAL` running it reports it.
//!
//! A `MATCH` types each arm as a block whose `it` is the scrutinee retyped to the arm's guard, keeps
//! the arms a run can take as static selection keeps a use's candidates, refusing a `MATCH` none of
//! whose arms can run or several of whose arms always admit with none ranking first, and is the
//! span of the kept arms' types ([`arms`]). A `MATCH … UNDER`'s arms are checked against its union
//! beside the guards, where the walk closes the union clause.
//!
//! A `MODULE` or `GROUP` binder's body is typed where its binder is, since it runs inline, and the
//! binder is exactly the signature the run ties where every member is exact ([`module_signature`]),
//! else at most `Module`. A `:|` ascription is a view: refused where every run's
//! view door refuses it — an operand that can never be a module, a closed type that is no one
//! application of a signature — or where an exact operand can never fit it; exactly the view's
//! signature over a tree the load sees all of, and at most the type otherwise. A `:!` is a retype,
//! held to its type as an annotation is; a module exact at a closed type is exactly the
//! application the run pins it at ([`pinned_at_fit`]), and refused where it fits none. A member
//! read `m.f` is typed by its [rule](super::rules), which reads the member off the operand's
//! signature, and refused where a signature its lower end is lacks the member. A value the run
//! retypes by its fit at a signature leaving a head parameter unpinned is typed exactly at that
//! signature pinned at the paths through the binder it is rooted at ([`paths`]). A
//! `USING … SCOPE` body is typed as a block, each surfaced name at the member read it names; a key
//! it surfaces is a spread typed by the one head its operand declares there, at most *maybe*, since
//! a view carries every overload its member admits.
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
//! lays every type the view door computes from types alone ([`exposure`](view::exposure)) in the
//! root, each nested module's view among them, read off what the underlying's tree shows each
//! member carries ([`views`]); a view of a view that shows a part its source hides is refused
//! there. At an exact `:!`, the types
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

use crate::elaborate::judge::{self, Marks, NoMarks};
use crate::elaborate::{
    SignatureMember, declares_nominal, module_signing, signature_member, tag_union,
};
use crate::knot::module::layout;
use crate::knot::module::view::{self, Ascription, Exposure, Operand};
use crate::knot::{BuiltinFunction, KBuiltins};
use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, Lays, bump_set, bump_table, collect,
    resident,
};
use crate::parse::BuiltinShapeId;
use crate::parse::{
    ExpressionPart, KExpression, KLiteral, ListItem, TemplateSlot, template_fields,
};
use crate::scope::{
    Address, Arms, Bare, Binder, BodyClass, BodyShape, Born, BuiltinIndex, Candidate,
    CandidateList, CandidateRoute, CaptureChain, CaptureSlot, CaptureSource, Closing,
    ComponentIndex, Coordinate, Cut, Elimination, Frontier, Guard, Identity, InstanceRead, Kept,
    Keyed, Lands, Leaf, Listed, NamedVariable, Narrowing, NeverSatisfied, Noted, PathEntry,
    PathName, Pick, Point, Points, Position, Resolution, Restamp, Retypes, Root, Route, Segment,
    ShapeError, ShapeKind, Shapes, Site, SiteDeps, Slot, Static, StaticInstance, StaticRegistered,
    StaticType, Store, SurfacedHead, Traced, Tree, Typed, UnitFacts, UnitId, UnitIndex, UnitRecord,
    UnitWork, Unknown, UseStatics, Variable as Located, Wake, Within, closed_under, follow,
    part_of, source_of, tag_projection, traced_quote,
};
use crate::scope::{Holder, Question, Rule, Run, Running, Surfacing, runs, solve_read};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, Interval, KKind, KType, Parametric, PathClass, Scheme,
    Side, Step, TypeNode, Variable, Verdict, instantiate_quantified, scheme_return, scheme_slots,
    shape_return, shape_slots, solving_slots,
};
use crate::values::{
    Chain, Restamping, Value, list_type, pinned_at_fit, record_type, retype_target, under, unknown,
};

mod arms;
mod caching;
mod catching;
mod lookups;
mod paths;
mod reads;
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

/// Walk the body at `at` of `shapes`, and every body nested in it, into `store`, interning through
/// `types`: the language's load at the root, or a resumption under the store's binding. A refusal
/// of written code stops the walk, worded by the store.
///
/// A walk rooted below the program first stands the chain around `at` as the whole walk would hold
/// it there: each enclosing body's level, holding its shape alone, entered as the whole walk enters
/// it. A read of a standing body's fact goes to the record of the one unit holding it, at or before
/// the unit birthing the next level, and gathers that record's frontier; nothing is copied, and no
/// table is sized to an encloser. Those units are never re-run: a binding below them wakes none of
/// them. So the walk costs `at`'s subtree and the facts it reads of the chain, however large the
/// program around them. A block or module body at `at` is walked as its encloser's unit would walk
/// it, over what the standing encloser's records say a `USING` holding it surfaces.
pub(super) fn walk<'graph: 'o, 'tier: 'o, 'o, S: Store<'o>>(
    shapes: &'graph Shapes<'graph>,
    builtins: &'graph KBuiltins<'graph, 'graph>,
    store: &S,
    types: &Chain<'tier, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    at: Segment,
) -> Result<(), ShapeError<'o>> {
    #[cfg(test)]
    let _walking = crate::elaborate::Walking::enter();
    let mut pass = Pass {
        shapes,
        builtins,
        types,
        scratch,
        store,
        lay: store.lay(),
        chain: BumpVec::new_in(scratch),
        units: BumpVec::new_in(scratch),
        started: bump_table(scratch),
        read: std::cell::RefCell::new(Read {
            units: BumpVec::new_in(scratch),
            tags: bump_table(scratch),
        }),
        open: BumpVec::new_in(scratch),
        scopes: std::cell::Cell::new(BumpVec::new_in(scratch)),
        walked: bump_set(scratch),
        demand: Demand {
            marks: bump_table(scratch),
            stack: BumpVec::new_in(scratch),
            next: 0,
            pending: BumpVec::new_in(scratch),
        },
        composed: bump_set(scratch),
        unfilled: false,
        absorbing: false,
        unnarrowed: unnarrowed_now(),
        trees: bump_table(scratch),
        reading: BumpVec::new_in(scratch),
        holding: bump_table(scratch),
        holders: bump_table(scratch),
        held: BumpVec::new_in(scratch),
        fixings: 0,
        reached: std::cell::RefCell::new(bump_table(scratch)),
        binders: bump_table(scratch),
        shaped: trees::Shaped::NONE,
        views: bump_table(scratch),
        view_trees: bump_table(scratch),
        steps: bump_table(scratch),
        rebuilds: bump_table(scratch),
        modules: bump_table(scratch),
        sides: bump_table(scratch),
        paths: BumpVec::new_in(scratch),
        composites: bump_table(scratch),
        module_mints: bump_table(scratch),
        unlisted: None,
        open_path: None,
        components: BumpVec::new_in(scratch),
        rebound: std::cell::RefCell::new(reads::Rebound {
            types: bump_table(scratch),
            units: bump_table(scratch),
            closings: bump_table(scratch),
            table: bump_table(scratch),
            trees: bump_table(scratch),
            classes: bump_table(scratch),
        }),
        leaf_binders: std::cell::RefCell::new(bump_table(scratch)),
    };
    let parent = pass.stand_chain(at);
    let shape = shapes.shape_at(at);
    match (shape.kind(), parent) {
        (ShapeKind::Block | ShapeKind::Module, Some(parent)) => pass.inline_root(parent, shape)?,
        _ => pass.body(parent, shape)?,
    }
    pass.finish_rooted();
    // The layer's table: each type a record read closed names, closed under the binding, and
    // each tree an absorbed record holds that the binding changes.
    let rebound = pass.rebound.into_inner();
    if !rebound.table.is_empty() || !rebound.trees.is_empty() {
        let mut keys = BumpVec::with_capacity_in(rebound.table.len(), scratch);
        keys.extend(rebound.table.keys().copied());
        let closed = closed_under(store.lay(), scratch, &keys, &mut |key| {
            (rebound.table.get(&key).copied()).unwrap_or(crate::scope::Closed::Same)
        });
        let mut trees = BumpVec::with_capacity_in(rebound.trees.len(), scratch);
        trees.extend(
            rebound
                .trees
                .iter()
                .map(|(identity, tree)| (*identity, *tree)),
        );
        store.write_closed(closed.with_trees(store.lay(), scratch, &trees));
    }
    Ok(())
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

/// Whether a value of the static type `typed` can reach a type minted at run: where its upper end
/// is porous or exposing, and, where that is exact and closed, where it is exposing — an exact
/// closed site carries that very type, and the surface door hides what the type does not name.
fn porous_site(types: &Chain<'_, '_, '_, '_>, typed: Interval) -> bool {
    // A fold's reading, which is the load's alone.
    match judge::exactly(types, typed) {
        Some(_) => types.exposing(typed.upper),
        None => porous_slot(types, typed.upper),
    }
}

/// Whether a value held to the declared type `slot` — a return, a parameter — can carry a type
/// minted at run.
fn porous_slot(types: &Chain<'_, '_, '_, '_>, slot: Parametric) -> bool {
    types.porous(slot) || types.exposing(slot)
}

/// What a view's sides memo holds: the sides beside the two types read, which a hit judges again
/// so its unit marks as the first did.
type SidesRead = (
    views::Sides,
    DeclaredType<Parametric>,
    DeclaredType<Parametric>,
);

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
/// in, which a read of the fact gathers the frontier of. One with [`READ`] set indexes the units
/// whose records the walk read without running or absorbing them ([`Read`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Tag(u32);

/// The bit a [`Tag`] of a unit the walk only read carries.
const READ: u32 = 1 << 31;

/// The units whose records a walk read without running or absorbing them — a standing body's, or
/// one a class's content is read off — each beside its record's frontier, which a read of one of
/// its facts gathers, and each one's tag. Kept apart from the walk's units, so a read door that
/// takes `&self` can tag one at its first read.
struct Read<'p, 'o> {
    units: BumpVec<'p, (UnitId, Frontier<'o>)>,
    /// By the unit, and by the binder where a read takes an entry unit's seed alone.
    tags: BumpBackedMap<'p, (UnitId, Option<Slot>), Tag>,
}

/// How the walk enters an inline body's parameters: as a `USING … SCOPE` body, each surfaced name
/// the member read of an operand of this static type, or as a `MATCH` arm, whose `it` lies within
/// this interval — each beside the unit that fixed it. A plain body binds no parameter.
#[derive(Clone, Copy)]
enum Entering {
    Plain,
    Surfaced(Interval, Tag),
    Arm(Interval, Tag),
}

/// Where a value held to a type is pinned at its paths
/// ([`at_its_paths`](Pass::at_its_paths)): nowhere, at the binder an annotated `LET` binds
/// (re-stamped at its declared type, since a point over the binder's own class needs the binder
/// bound to stamp it), or in place at its operand's paths (re-stamped at the pinned point).
#[derive(Clone, Copy)]
enum PinnedAt<'c> {
    Nowhere,
    Binder(Binder),
    Operand(Binder, &'c [ValueSymbol]),
}

/// One running unit's gathering: each unknown it read itself beside why, and each ended frontier
/// it read, cut to the chain as it read it. Unsorted until its frontier lays it.
struct Gathering<'p, 'o> {
    noted: BumpVec<'p, Noted<'o>>,
    reads: BumpVec<'p, Cut<'o>>,
}

impl<'p> Gathering<'p, '_> {
    fn new(scratch: BumpAllocator<'p>) -> Self {
        Gathering {
            noted: BumpVec::new_in(scratch),
            reads: BumpVec::new_in(scratch),
        }
    }
}

/// One unit the walk ran or absorbed, or one owning body's closing.
struct UnitState<'p, 'o> {
    id: UnitId,
    /// Once it ends: its record's frontier. While it runs, its gathering is the walk's.
    frontier: Option<Frontier<'o>>,
    /// What its record keeps beside its body's own facts: the signature of each module body it
    /// ran; each callable or quote's code it births; each path it resolved first on its chain;
    /// what the folds read.
    typed: BumpVec<'p, (Segment, Typed<'o>)>,
    born: BumpVec<'p, (Segment, Born<'o>)>,
    paths: BumpVec<'p, ((Segment, PathName), (PathEntry<'o>, Resolution<'o>))>,
    sites: BumpVec<'p, SiteDeps<'o>>,
    named_variables: BumpVec<'p, NamedVariable>,
    /// An entry unit's seed of each binder, by slot.
    seeds: BumpVec<'p, (Slot, Frontier<'o>)>,
    /// A seed's entry unit, whose record keeps what the seed fixes.
    owner: Option<Tag>,
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
            seeds: BumpVec::new_in(scratch),
            owner: None,
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
    Traced(Site),
    Settled(usize),
    Minting(usize),
    Instance(usize),
    Named(usize),
    Route(usize),
    Arms(usize),
    Retype(usize),
    Tree(usize),
    Picks(usize),
    Content(usize),
}

/// One shape on the chain, innermost last, with what the walk has fixed of it so far, each fact
/// beside the unit that fixed it.
struct Level<'p, 'o, 'graph> {
    shape: &'graph BodyShape<'graph>,
    /// Whether the shape roots a chain: the program or a quote's code.
    roots_chain: bool,
    /// Where the shape encloses a rooted walk's root: the unit birthing the next level. Such a
    /// **standing** level is read off its records up to that unit, never walked.
    standing: Option<UnitIndex>,
    /// The index in `Pass::paths` of the table of the chain the shape sits on.
    paths: usize,
    /// The type half's facts: each type binder's declaration and each registration's entry, by
    /// slot, and each recorded type expression's type, by site.
    declared: BumpVec<'p, Option<(StaticType<'o>, Tag)>>,
    registered: BumpVec<'p, Option<(StaticRegistered<'o>, Tag)>>,
    expressions: BumpBackedMap<'p, Site, (StaticType<'o>, Tag)>,
    /// Each part's static type, by site: its latest where the walk typed it twice.
    parts: BumpBackedMap<'p, Site, (Interval, Tag)>,
    /// Each part's [peek](Pass::peeked), by site, made once, beside the gathering it noted into: a
    /// part is peeked only while its statement's unit runs, so the miss already noted what it read
    /// into the gathering a hit would note into.
    peeks: BumpBackedMap<'p, Site, (Option<Interval>, Option<Tag>)>,
    /// Whether each part is a value the frame body [received](Pass::received), by site, read once:
    /// it reads only the chain's shapes and the builtins, which stay put while the level stands.
    received: BumpBackedMap<'p, Site, bool>,
    statements: BumpVec<'p, Option<(Interval, Tag)>>,
    binders: BumpVec<'p, (Bound, Option<Tag>)>,
    /// What the walk fixed about each keyworded use, by index into the shape's candidate lists.
    uses: BumpVec<'p, (UseStatics<'o>, Option<Tag>)>,
    /// Each keyworded use the walk selected a candidate at, by its node's site, beside the closed
    /// point each variable of the candidate's group solves to there, by group index.
    selections: BumpBackedMap<'p, Site, (Points<'o>, Tag)>,
    /// Each selected use of a callable body, by its node's site, beside what the site fixes of
    /// its callee's names.
    traced: BumpBackedMap<'p, Site, (Traced<'o>, Tag)>,
    /// Each `:!` whose operand's static upper end lies under its type, and each annotated binder's
    /// type part whose value's does.
    settled: BumpVec<'p, (Site, Tag)>,
    /// Each site where a quantified function is read, beside what the load fixed there.
    instances: BumpVec<'p, (Site, InstanceRead<'o>, Tag)>,
    /// Each call by name's contributions, by its argument part's site: per parameter in symbol
    /// order.
    named: BumpVec<'p, (Site, &'o [StaticType<'o>], Tag)>,
    /// Each tail call's route, by its node's site.
    routes: BumpVec<'p, (Site, Route<'o>, Tag)>,
    /// Each `MATCH`'s arms, by its node's site.
    arms: BumpVec<'p, (Site, Arms<'o>, Tag)>,
    /// Each retype to a closed declared type, by its settled site or member read's node site.
    retypes: BumpVec<'p, (Site, Restamp, Tag)>,
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
    picks: BumpVec<'p, (Site, &'o [Pick<'o>], Tag)>,
    /// Each module binder's content tree, by slot: what a binding of it is read against.
    /// Each one's held identities beside it ([`Pass::holding`]).
    contents: BumpVec<'p, (Slot, Tree<'o>, &'o [Identity], Tag)>,
    /// A `MODULE` or `GROUP` body's exact signature, written over names, where the walk typed one.
    signature: Option<Parametric>,
    /// The statement the walk is typing: where it looks for a part first.
    statement: usize,
    /// A `MODULE` body's builder captures' trees, resolved once a leaf on one of its binders is
    /// keyed on them.
    captures: Option<&'o [(u32, Tree<'o>)]>,
}

impl Level<'_, '_, '_> {
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
    marks: BumpBackedMap<'p, Segment, Mark>,
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
    started: BumpBackedMap<'p, UnitId, Tag>,
    read: std::cell::RefCell<Read<'p, 'o>>,
    /// The units running, innermost last, beside each one's gathering: every unknown and ended
    /// frontier its facts read so far, unsorted until its frontier lays it.
    open: BumpVec<'p, Tag>,
    scopes: std::cell::Cell<BumpVec<'p, Gathering<'p, 'o>>>,
    /// Each owning body the walk has entered, by segment.
    walked: BumpBackedSet<'p, Segment>,
    demand: Demand<'p, 'o>,
    /// The levels of the type captures each body composes so far, beside its segment: a lexical
    /// variable a contribution or a solution names, read from inside the body and homed outside it.
    composed: BumpBackedSet<'p, (Segment, usize)>,
    /// Whether the walk is typing a quote's code as a traced `EVAL` runs it, no hole filled: it
    /// fixes nothing.
    unfilled: bool,
    /// Whether the walk is reaching the inline bodies of an absorbed unit, whose units' records
    /// are cut into its own: each is absorbed with it ([`absorbed_births`](Pass::absorbed_births)).
    absorbing: bool,
    /// Whether the walk leaves every keyworded use whole and refuses none: the narrowing law's
    /// switch.
    unnarrowed: bool,
    /// Each content tree read on the chain, by its shape's address and where it is written.
    trees: BumpBackedMap<'p, (usize, Written), (Tree<'o>, Tag)>,
    /// Each binder whose tree is being read, by its shape's address: a binder read through itself
    /// reads as a leaf.
    reading: BumpVec<'p, (usize, Slot)>,
    /// Each module binder whose content tree the walk fixed, and each view written in place whose
    /// tree it recorded, by its shape and its holder: the tree, the unit that fixed it, and the
    /// identity of each module it holds, its own and a binder's members'.
    holding: BumpBackedMap<'p, (Segment, Holder), (Tree<'o>, Tag, &'o [Identity])>,
    /// Each holder holding a module of a tree of some identity, by its shape and that identity:
    /// the newest of [`held`](Self::held)'s entries for them. Where a class's variable is read
    /// through a root holding a module of its tree.
    holders: BumpBackedMap<'p, (Segment, Identity), u32>,
    /// Each holder `holders` lists, beside the index of the one listed before it.
    held: BumpVec<'p, (Holder, Option<u32>)>,
    /// How many module binders' content trees the walk fixed so far: a binder fixed again, to a
    /// tree holding nothing, counts.
    fixings: u32,
    /// Each class the walk asked a shape to reach, by the shape, the class and how many content
    /// trees the walk had fixed then ([`fixings`](Self::fixings)), beside the answer and the units
    /// the search gathered: a shape reaches a class alike until another content tree is fixed.
    reached: std::cell::RefCell<BumpBackedMap<'p, Reach, Reached<'p, 'o>>>,
    /// Each binder's tree, by its shape's address, its slot and, read through a body's tree, the
    /// address of that tree's captures: built once per binding.
    binders: BumpBackedMap<'p, (usize, Slot, Option<usize>), (Tree<'o>, Tag)>,
    /// What shaped the binder tree being built besides the program's text.
    shaped: trees::Shaped,
    /// What each view over a closed tree binds, by its identity: a chain of views reads each once.
    views: BumpBackedMap<'p, Identity, (Option<Exposure<'o, 'p>>, Tag)>,
    /// What each opaque view's fit solves, by its application and its source's identity, beside the
    /// type its source carries: a `:|` fits its source, and reads that type, once.
    view_trees: BumpBackedMap<'p, (KType, Identity), trees::Fitted>,
    /// What each read of a value a lay may reach twice yields, so a chain of views reads each
    /// member once.
    steps: BumpBackedMap<'p, (views::ViewIdentity, views::Step), Option<views::Carried<'o>>>,
    /// Each value a view rebuilt, by what it was rebuilt from.
    rebuilds: BumpBackedMap<'p, views::Rebuild, views::Carried<'o>>,
    /// The modules each rebuilt union of cells' signatures may be, cell by cell.
    modules: BumpBackedMap<'p, views::ViewIdentity, &'p [views::Carried<'o>]>,
    /// Each declared type a view reads either side, by its two sides' handles.
    sides: BumpBackedMap<'p, (DeclaredType<Parametric>, KType, KType), SidesRead>,
    /// The path table of each chain being typed, outermost first.
    paths: BumpVec<'p, paths::ChainPaths<'p, 'o, 'graph>>,
    /// Each composite site's dependencies, by its body's segment and its site: a part that is one
    /// takes its class.
    composites: BumpBackedMap<'p, (Segment, Site), caching::Reads<'o>>,
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
    /// What reading the records the binding names but does not wake closed keeps.
    rebound: std::cell::RefCell<reads::Rebound<'p, 'o>>,
    /// Each binder a leaf of an open tree reads in the run being typed, by the tree's identity.
    leaf_binders: std::cell::RefCell<BumpBackedMap<'p, Identity, &'p [Binder]>>,
}

/// A reach the walk keeps ([`Pass::reached`]): the reading shape, the class, and how many content
/// trees the walk had fixed when it searched.
type Reach = (Segment, PathClass, u32);

/// A kept reach's answer, beside the units its search gathered.
type Reached<'p, 'o> = (Option<paths::Witness<'o>>, &'p [Tag]);

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
    /// Whether the candidate is a surfaced key's closed signature head, whose frame retypes its
    /// value to the head's return.
    retyped: bool,
    verdict: Verdict,
    intervals: Option<&'x [Interval]>,
    ruled: Option<Interval>,
    /// The instance each of the use's instance arguments takes at this candidate, in argument
    /// order.
    instances: &'x [Made<'x>],
}

/// A unit's statements in program order, beside the callables, quotes' code and blocks they nest,
/// each in program order: collected once per unit.
#[derive(Clone, Copy)]
struct UnitStatements<'p, 'graph> {
    statements: &'p [usize],
    callables: &'p [&'graph BodyShape<'graph>],
    codes: &'p [&'graph BodyShape<'graph>],
    blocks: &'p [&'graph BodyShape<'graph>],
}

/// What a keyworded use reads of one candidate, read once per use: what the load knows of its
/// registered shape, the heads it holds where it is a `USING … SCOPE` block's surfaced key, and the
/// slots its shape's solve reads, empty where the load does not know the shape.
#[derive(Clone, Copy)]
struct CandidateRead<'p, 'graph> {
    known: Known,
    surfaced: Option<&'graph [SurfacedHead<'graph>]>,
    solving: &'p [bool],
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

/// A call by name's argument laid onto its callee's slots: the argument's static type, each slot's
/// static type, and each slot's contribution.
type Laid<'p, 'o> = (Interval, &'p [Interval], &'o [StaticType<'o>]);

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
    /// The slots `shape`'s solve reads.
    solving: &'p [bool],
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

    fn push(
        &mut self,
        shape: &'graph BodyShape<'graph>,
        roots_chain: bool,
        standing: Option<UnitIndex>,
    ) {
        debug_assert_eq!(
            self.shapes
                .parent(shape.segment())
                .map(|(parent, _)| parent),
            self.chain.last().map(|level| level.shape.segment()),
            "the chain is a root path: each level the body enclosing the next"
        );
        let scratch = self.scratch;
        // A standing level holds its shape alone: every read of it goes to its records.
        let (body, slots, lists) = match standing {
            Some(_) => (0, 0, 0),
            None => (
                shape.body().len(),
                shape.slots(),
                shape.candidate_lists().len(),
            ),
        };
        let mut statements = BumpVec::with_capacity_in(body, scratch);
        statements.resize(body, None);
        let mut binders = BumpVec::with_capacity_in(slots, scratch);
        binders.resize(slots, (DeclaredType::Type(unknown()), None));
        let mut declared = BumpVec::with_capacity_in(slots, scratch);
        declared.resize(slots, None);
        let mut registered = BumpVec::with_capacity_in(slots, scratch);
        registered.resize(slots, None);
        let mut uses = BumpVec::with_capacity_in(lists, scratch);
        uses.resize(lists, (UseStatics::UNFIXED, None));
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
            standing,
            paths,
            declared,
            registered,
            expressions: bump_table(scratch),
            parts: bump_table(scratch),
            peeks: bump_table(scratch),
            received: bump_table(scratch),
            statements,
            binders,
            uses,
            selections: bump_table(scratch),
            traced: bump_table(scratch),
            settled: BumpVec::new_in(scratch),
            instances: BumpVec::new_in(scratch),
            named: BumpVec::new_in(scratch),
            routes: BumpVec::new_in(scratch),
            arms: BumpVec::new_in(scratch),
            retypes: BumpVec::new_in(scratch),
            exits: false,
            minting: BumpVec::new_in(scratch),
            closed: 0,
            folded: 0,
            minted_within: false,
            trees: BumpVec::new_in(scratch),
            picks: BumpVec::new_in(scratch),
            contents: BumpVec::new_in(scratch),
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
        if !self.walked.insert(segment) {
            return Ok(());
        }
        // An owning body's units are its own, whatever absorbed the unit that births it.
        let absorbing = std::mem::replace(&mut self.absorbing, false);
        let walked = self.owning_body(parent, shape);
        self.absorbing = absorbing;
        walked
    }

    /// [`body`](Self::body) once the body is entered.
    fn owning_body(
        &mut self,
        parent: Option<usize>,
        shape: &'graph BodyShape<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let segment = shape.segment();
        let code = shape.kind() == ShapeKind::Code;
        let callable = shape.kind() == ShapeKind::Callable;
        if callable {
            self.enter(segment);
        }
        let (depth, open) = (self.chain.len(), self.open.len());
        let roots_chain = parent.is_none();
        self.push(shape, roots_chain, None);
        let level = self.chain.len() - 1;
        if roots_chain {
            self.open_paths(shape);
        }
        // A quote whose code could not be built has nothing to type.
        let walked = match code && shape.refusal().is_some() {
            true => Err(*shape
                .refusal()
                .expect("a refused code shape holds its refusal")),
            false => (self.units_of(level, Entering::Plain)).and_then(|_| self.closing(level)),
        };
        // A quote's code keeps its first overlap for the `EVAL` running it to report.
        let walked = walked.map(|(mut closing, tag)| {
            if code {
                closing.overlap = self.overlap_in(shape);
            }
            (closing, tag)
        });
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
                self.note_named(segment, |each| closing.each_type(each));
                (closing, self.end(tag))
            }
            Err(error) if code => {
                // A refused quote's rules closed nothing, but a binding may change its refusal.
                self.unreached(shape);
                let frontier = self.unwound(open, shape);
                let refused = Closing {
                    class: None,
                    summary: None,
                    recursive: false,
                    ran: None,
                    refusal: Some(resident(self.lay, error)),
                    overlap: None,
                    retypes: Retypes::NONE,
                };
                (refused, frontier)
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

    /// Stand the chain of bodies around `at`, from the program down, as the whole walk holds it when
    /// it reaches `at`, each level its shape alone, marked with the unit birthing the next: the
    /// chain level `at` is nested in, `None` at the program.
    fn stand_chain(&mut self, at: Segment) -> Option<usize> {
        let mut around = BumpVec::new_in(self.scratch);
        let mut child = at;
        while let Some((parent, statement)) = self.shapes.parent(child) {
            around.push((parent, statement));
            child = parent;
        }
        for (segment, statement) in around.into_iter().rev() {
            let shape = self.shape_at(segment);
            let inline = matches!(shape.kind(), ShapeKind::Block | ShapeKind::Module);
            if !inline {
                self.walked.insert(segment);
            }
            if shape.kind() == ShapeKind::Callable {
                self.enter(segment);
            }
            let roots_chain = self.chain.is_empty();
            let birth = shape.unit_of_statement(statement as usize);
            self.push(shape, roots_chain, Some(birth));
            if roots_chain {
                self.open_paths(shape);
            }
        }
        self.chain.len().checked_sub(1)
    }

    /// Walk each callable born inside the inline bodies the statements `statements` of the shape at
    /// `level` run, where their unit was absorbed: the walk did not run it, so it did not reach
    /// them, and a binding may wake their units still.
    fn absorbed_births(
        &mut self,
        level: usize,
        statements: impl Iterator<Item = usize>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        for index in statements {
            for (_, nested) in shape.nested_in(index) {
                if !matches!(nested.kind(), ShapeKind::Block | ShapeKind::Module) {
                    continue;
                }
                // Its units' frontiers are cut into the absorbed unit's, which reads each, so each
                // is absorbed with it, and the callables they birth walked.
                self.push(nested, false, None);
                let at = self.chain.len() - 1;
                let absorbing = std::mem::replace(&mut self.absorbing, true);
                let walked = self.units_of(at, Entering::Plain);
                self.absorbing = absorbing;
                walked?;
                self.chain.pop();
            }
        }
        Ok(())
    }

    /// Walk the inline body `shape`, the root of a walk, nested in the standing shape at `level`: its
    /// units, its entry over what the encloser's records say a `USING` holding it surfaces.
    fn inline_root(
        &mut self,
        level: usize,
        shape: &'graph BodyShape<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let entering = self.entering_of(level, shape);
        self.push(shape, false, None);
        let at = self.chain.len() - 1;
        self.units_of(at, entering)?;
        self.chain.pop();
        Ok(())
    }

    /// How the shape at `level` entered the inline body `block`, as its facts have it: as a
    /// `USING … SCOPE` body over its operand's static type, or as a `MATCH` arm over the interval
    /// the `MATCH` typed its `it` at, which it holds at the arm's site; each beside the unit that
    /// fixed it.
    fn entering_of(&self, level: usize, block: &BodyShape<'graph>) -> Entering {
        let shape = self.chain[level].shape;
        let Some((_, statement)) = self.shapes.parent(block.segment()) else {
            return Entering::Plain;
        };
        if block.arm().is_some() {
            let site = (shape.nested_in(statement as usize).iter())
                .find_map(|(site, nested)| (nested.segment() == block.segment()).then_some(*site))
                .expect("an arm is nested in its statement");
            return match self.part_held(level, site) {
                Some((typed, tag)) => Entering::Arm(typed, tag),
                None => Entering::Plain,
            };
        }
        let holding = &shape.body()[statement as usize];
        let surfaces = |node: &'graph KExpression<'graph>| {
            matches!(of_node(shape, node), Form::Using(_))
                && (shape.site_of_body(node).and_then(|site| shape.nested(site)))
                    .is_some_and(|body| body.segment() == block.segment())
        };
        let mut using = surfaces(holding).then_some(holding);
        crate::scope::each_part(holding, |part| {
            if let ExpressionPart::Expression(node) = part
                && surfaces(node)
            {
                using = Some(node);
            }
        });
        let Some(using) = using else {
            return Entering::Plain;
        };
        let [_, operand, _, _] = using.parts else {
            unreachable!("a `USING … SCOPE` has its keyword, its module, `SCOPE` and a body")
        };
        let site = self.site(level, &operand.value);
        match self.part_held(level, site) {
            Some((typed, tag)) => Entering::Surfaced(typed, tag),
            None => Entering::Plain,
        }
    }

    /// Write the closings a rooted walk left pending: each of a component reaching a callable on the
    /// chain around the walk's root, which the walk never finishes. Such a component holds that
    /// callable too, so it is recursive.
    fn finish_rooted(&mut self) {
        for (segment, mut closing, frontier) in
            std::mem::replace(&mut self.demand.pending, BumpVec::new_in(self.scratch))
        {
            closing.recursive = true;
            closing.summary = Some(None);
            self.write_closing(segment, closing, frontier);
        }
    }

    /// The first registration of the quote's code `shape`, or of a body nested in it short of
    /// another quote's code, that overlaps a builtin overload, each as the walk typed it.
    fn overlap_in(&self, shape: &'graph BodyShape<'graph>) -> Option<&'o ShapeError<'o>> {
        let typed = |body: &BodyShape<'graph>, slot| self.registered_cell(body, slot);
        let overlaps = super::check::overlaps_through(
            self,
            shape,
            &typed,
            self.builtins,
            self.types,
            self.scratch,
        );
        overlaps.err().map(|error| resident(self.lay, error))
    }

    /// Run the entry and each unit of the body at `level`, each callable a unit births walked
    /// right after it: an inline body's entry as `entering` says.
    fn units_of(&mut self, level: usize, entering: Entering) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        self.unit(level, UnitIndex::ENTRY, |pass| pass.entry(level, entering))?;
        for (index, unit) in shape.units().iter().enumerate() {
            let at = UnitIndex::of_run(index);
            // Collected outside the unit: one the store absorbs runs no steps but births its
            // callables.
            let nests = self.unit_statements(shape, unit.work);
            let ran = self.unit(level, at, |pass| pass.steps(level, at, unit.work, nests))?;
            if !ran {
                self.absorbed_births(level, nests.statements.iter().copied())?;
                // An absorbed unit's codes are walked as its callables are: their records name
                // the chain's levels, so each stands under the binding, its types in its table.
                for code in nests.codes {
                    self.body(Some(level), code)?;
                }
            }
            for born in nests.callables {
                self.body(Some(level), born)?;
            }
        }
        Ok(())
    }

    /// The statements of `shape` the unit doing `work` runs, and what they nest.
    fn unit_statements(
        &self,
        shape: &BodyShape<'graph>,
        work: UnitWork,
    ) -> UnitStatements<'p, 'graph> {
        let scratch = self.scratch;
        let mut statements = BumpVec::new_in(scratch);
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
        let (mut callables, mut codes, mut blocks) = (
            BumpVec::new_in(scratch),
            BumpVec::new_in(scratch),
            BumpVec::new_in(scratch),
        );
        for index in statements.iter() {
            for (_, nested) in shape.nested_in(*index) {
                match nested.kind() {
                    ShapeKind::Callable => callables.push(*nested),
                    ShapeKind::Code => codes.push(*nested),
                    ShapeKind::Block => blocks.push(*nested),
                    _ => {}
                }
            }
        }
        UnitStatements {
            statements: statements.leak(),
            callables: callables.leak(),
            codes: codes.leak(),
            blocks: blocks.leak(),
        }
    }

    /// Run `run` as the unit `index` of the owning body at `level`, inside a gathering of its own,
    /// and write its record; or absorb the record the store holds where its binding does not wake
    /// it. Whether it ran. The unfilled typing of a quote's code keeps no record: its reads go to
    /// the closing gathering it runs in.
    fn unit(
        &mut self,
        level: usize,
        index: UnitIndex,
        run: impl FnOnce(&mut Self) -> Result<(), ShapeError<'o>>,
    ) -> Result<bool, ShapeError<'o>> {
        if self.unfilled {
            return run(self).map(|()| true);
        }
        let id = UnitId {
            segment: self.chain[level].shape.segment(),
            unit: index,
        };
        #[cfg(test)]
        if let Some(visit) = VISIT.with(std::cell::Cell::get) {
            visit(id);
        }
        if let Some((facts, frontier)) = self.store.fact(id)
            && (self.absorbing || self.store.wakes(frontier, self.types) != Wake::Open)
        {
            if self.store.wakes(frontier, self.types) == Wake::Named {
                // Its trees are substituted in a gathering of their own, which the record's
                // frontier already covers: the binding's layer alone holds what they read.
                let depth = self.open.len();
                self.open_unit(id, true);
                let substituted = self.substitute_trees(facts);
                self.unwind(depth);
                substituted?;
            }
            let facts = self.standing_record(id, facts, frontier);
            self.absorb(level, id, facts, frontier);
            return Ok(false);
        }
        #[cfg(test)]
        if let Some((_, frontier)) = self.store.fact(id)
            && !self.store.traces()
        {
            self.count_rerun(frontier);
        }
        let tag = self.open_unit(id, false);
        run(self)?;
        let facts = self.record(level, tag);
        // A record names each variable its types hold.
        self.note_named(id.segment, |each| facts.each_type(each));
        let frontier = self.end(tag);
        #[cfg(test)]
        tally(frontier);
        self.store.write(id, UnitRecord::Typed(facts, frontier));
        Ok(true)
    }

    /// Open a gathering for `id`, a unit or — `closing` — a body's closing.
    fn open_unit(&mut self, id: UnitId, closing: bool) -> Tag {
        let scratch = self.scratch;
        let tag = Tag(self.units.len() as u32);
        self.units.push(UnitState::new(id, None, scratch));
        if !closing {
            self.started.insert(id, tag);
        }
        self.open.push(tag);
        let mut scopes = self.scopes.replace(BumpVec::new_in(scratch));
        scopes.push(Gathering::new(scratch));
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
        let frontier = Frontier::laid(self.lay, &mut gathered.noted, &gathered.reads);
        #[cfg(test)]
        count(|counts| counts.frontier_entries += frontier.own().len() + frontier.reads().len());
        self.units[tag.0 as usize].frontier = Some(frontier);
        frontier
    }

    /// Mark unreached each unit of the refused code `code`, and of each body it nests, that this
    /// walk neither ended nor absorbed while a walk beneath recorded it: the unit refusing and those
    /// past it. A fresh walk under this binding records none of them, so no reader may find one.
    fn unreached(&self, code: &BodyShape<'graph>) {
        if self.unfilled {
            return;
        }
        let enclosed = (self.shapes.bodies().iter())
            .filter(|shape| self.shapes.encloses(code.segment(), shape.segment()));
        for shape in enclosed {
            let units = std::iter::once(UnitIndex::ENTRY)
                .chain((0..shape.units().len()).map(UnitIndex::of_run));
            for unit in units {
                let id = UnitId {
                    segment: shape.segment(),
                    unit,
                };
                let reached = (self.started.get(&id))
                    .is_some_and(|tag| self.units[tag.0 as usize].frontier.is_some());
                if !reached && self.store.fact(id).is_some() {
                    self.store.write(id, UnitRecord::Unreached);
                }
            }
        }
    }

    /// Unwind the gatherings opened past `depth` as [`unwind`](Self::unwind) does, handing back
    /// what they gathered beside the frontiers of the units of `code` that ended before its
    /// refusal: the refused closing's frontier, so a binding changing a question the code's typing
    /// reached walks the code again, and one naming its types closes them.
    fn unwound(&mut self, depth: usize, code: &BodyShape<'graph>) -> Frontier<'o> {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        let mut merged = Gathering::new(self.scratch);
        while self.open.len() > depth {
            let tag = self.open.pop().expect("a gathering is open");
            if let Some(gathered) = scopes.pop() {
                merged.noted.extend(gathered.noted);
                merged.reads.extend(gathered.reads);
            }
            self.units[tag.0 as usize].frontier = Some(Frontier::EMPTY);
        }
        self.scopes.set(scopes);
        // A code walked as the walk's root has no encloser on the chain: its own cut is its own.
        let from = match self.chain.is_empty() {
            true => code.segment(),
            false => self.meet(code.segment()),
        };
        let units =
            std::iter::once(UnitIndex::ENTRY).chain((0..code.units().len()).map(UnitIndex::of_run));
        for unit in units {
            let id = UnitId {
                segment: code.segment(),
                unit,
            };
            if let Some((_, frontier)) = self.store.fact(id)
                && !frontier.is_empty()
            {
                merged.reads.push(Cut { frontier, from });
            }
        }
        Frontier::laid(self.lay, &mut merged.noted, &merged.reads)
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
        let own = self.typed(level, tag);
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
            seeds: Keyed::sorted(lay, &mut state.seeds),
        };
        resident(lay, facts)
    }

    /// The facts of the shape at `level` the unit `tag` fixed, laid down, from where it wrote them.
    /// The writes sort by kind, then by key, so each table keyed as its writes are comes out
    /// sorted.
    fn typed(&mut self, level: usize, tag: Tag) -> Typed<'o> {
        let (lay, scratch) = (self.lay, self.scratch);
        let unit = &mut self.units[tag.0 as usize];
        let mut wrote = std::mem::replace(&mut unit.wrote, BumpVec::new_in(scratch));
        wrote.sort_unstable();
        wrote.dedup();
        let (exits, closed, folded) = (unit.exits, unit.closed, unit.folded);
        let at = &self.chain[level];
        let units = &self.units;
        let ours = |held: Tag| {
            held == tag || (held.0 & READ == 0 && units[held.0 as usize].owner == Some(tag))
        };
        let lists = at.shape.candidate_lists();
        let mut declared = BumpVec::new_in(scratch);
        let mut registered = BumpVec::new_in(scratch);
        let mut binders = BumpVec::new_in(scratch);
        let mut statements = BumpVec::new_in(scratch);
        let mut uses = BumpVec::new_in(scratch);
        let mut parts = BumpVec::new_in(scratch);
        let mut expressions = BumpVec::new_in(scratch);
        let mut selections = BumpVec::new_in(scratch);
        let mut traced = BumpVec::new_in(scratch);
        let mut settled = BumpVec::new_in(scratch);
        let mut minting = BumpVec::new_in(scratch);
        let mut instances = BumpVec::new_in(scratch);
        let mut named = BumpVec::new_in(scratch);
        let mut routes = BumpVec::new_in(scratch);
        let mut arms = BumpVec::new_in(scratch);
        let mut retypes = BumpVec::new_in(scratch);
        let mut trees = BumpVec::new_in(scratch);
        let mut picks = BumpVec::new_in(scratch);
        let mut contents = BumpVec::new_in(scratch);
        let mut holding = BumpVec::new_in(scratch);
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
                Wrote::Traced(site) => {
                    if let Some((call, held)) = at.traced.get(&site)
                        && ours(*held)
                    {
                        traced.push((site, *call));
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
                Wrote::Route(index) => {
                    let (site, route, held) = at.routes[index];
                    if ours(held) {
                        routes.push((site, route));
                    }
                }
                Wrote::Arms(index) => {
                    let (site, fixed, held) = at.arms[index];
                    if ours(held) {
                        arms.push((site, fixed));
                    }
                }
                Wrote::Retype(index) => {
                    let (site, retype, held) = at.retypes[index];
                    if ours(held) {
                        retypes.push((site, retype));
                    }
                }
                Wrote::Tree(index) => {
                    let (site, tree, held) = at.trees[index];
                    if ours(held) {
                        trees.push((site, tree));
                    }
                }
                Wrote::Picks(index) => {
                    let (site, picked, held) = at.picks[index];
                    if ours(held) {
                        picks.push((site, picked));
                    }
                }
                Wrote::Content(index) => {
                    let (slot, tree, identities, held) = at.contents[index];
                    if ours(held) {
                        contents.push((slot, tree));
                        holding.push((slot, identities));
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
        newest(&mut routes);
        newest(&mut arms);
        newest(&mut retypes);
        newest(&mut trees);
        newest(&mut picks);
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
            traced: keyed(lay, &traced),
            by_name: Keyed::sorted(lay, &mut named),
            routes: Keyed::sorted(lay, &mut routes),
            arms: Keyed::sorted(lay, &mut arms),
            retypes: Keyed::sorted(lay, &mut retypes),
            trees: Keyed::sorted(lay, &mut trees),
            picks: Keyed::sorted(lay, &mut picks),
            contents: Keyed::sorted(lay, &mut contents),
            holding: Keyed::sorted(lay, &mut holding),
            minting,
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
        self.started.insert(id, tag);
        let segment = self.chain[level].shape.segment();
        if let Some((_, typed)) = facts.typed.get(segment) {
            self.restore(level, typed, tag);
        }
        // A seeded binder is fixed by its seed, which a read of it gathers alone.
        for (slot, seed) in facts.seeds.iter() {
            let seeded = Tag(self.units.len() as u32);
            let mut state = UnitState::new(id, Some(*seed), self.scratch);
            state.owner = Some(tag);
            self.units.push(state);
            self.chain[level].binders[slot.index()].1 = Some(seeded);
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
        for (site, call) in typed.traced.iter() {
            at.traced.insert(*site, (*call, tag));
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
        at.routes.extend(
            typed
                .routes
                .iter()
                .map(|(site, route)| (*site, *route, tag)),
        );
        at.arms
            .extend(typed.arms.iter().map(|(site, arms)| (*site, *arms, tag)));
        at.retypes.extend(
            typed
                .retypes
                .iter()
                .map(|(site, retype)| (*site, *retype, tag)),
        );
        at.minting
            .extend(typed.minting.iter().map(|site| (*site, tag)));
        at.trees
            .extend(typed.trees.iter().map(|(site, tree)| (*site, *tree, tag)));
        at.picks.extend(
            typed
                .picks
                .iter()
                .map(|(site, picked)| (*site, *picked, tag)),
        );
        at.exits |= typed.exits;
        at.closed += typed.closed;
        at.folded += typed.folded;
        // A module binder's content tree stands with its unit, its body typed when it was.
        let (shape, segment) = (address(at.shape), at.shape.segment());
        for (slot, tree) in typed.contents.iter() {
            self.binders
                .entry((shape, *slot, None))
                .or_insert((*tree, tag));
            let identities = (typed.holding.get(*slot)).map_or(&[][..], |(_, held)| *held);
            self.hold(segment, Holder::Slot(*slot), *tree, identities, tag);
        }
        for (site, tree) in typed.trees.iter() {
            self.hold_view(segment, *site, tree, tag);
        }
    }

    /// Note that `holder` of `segment` holds `tree`, a module of each of `identities`, as the unit
    /// `tag` fixed it.
    fn hold(
        &mut self,
        segment: Segment,
        holder: Holder,
        tree: Tree<'o>,
        identities: &'o [Identity],
        tag: Tag,
    ) {
        self.fixings += 1;
        self.holding
            .insert((segment, holder), (tree, tag, identities));
        for identity in identities {
            let index = self.held.len() as u32;
            let before = self.holders.insert((segment, *identity), index);
            self.held.push((holder, before));
        }
    }

    /// Note that the view the `:|` at `site` of `segment` builds in place holds a module of its
    /// own tree `tree`, as the unit `tag` fixed it.
    fn hold_view(&mut self, segment: Segment, site: Site, tree: &'o Tree<'o>, tag: Tag) {
        let identity = tree.identity();
        if identity == Identity::Unnamed {
            return;
        }
        let identities = std::slice::from_ref(resident(self.lay, identity));
        self.hold(segment, Holder::Site(site), *tree, identities, tag);
    }

    /// The identity of each module `tree`, a module binder's content tree, holds: its own, then
    /// each one a value member of a module body it holds holds, as the body's units recorded the
    /// member's content tree. Read raw: a member's tree is the body's text over the captures
    /// `tree`'s identity keys, so it is fixed wherever `tree` is, and the body ran before its
    /// binder was bound; a reader naming the chain gathers what it reads.
    fn held_identities(&self, tree: Tree<'o>) -> &'o [Identity] {
        let mut held = BumpVec::new_in(self.scratch);
        let mut bodies = BumpVec::new_in(self.scratch);
        let mut pending = BumpVec::new_in(self.scratch);
        pending.push(tree);
        while let Some(tree) = pending.pop() {
            let identity = tree.identity();
            if identity == Identity::Unnamed || held.contains(&identity) {
                continue;
            }
            held.push(identity);
            let Tree::Body { body, .. } = tree else {
                continue;
            };
            // A body holding itself through a member holds nothing more.
            if bodies.contains(&body) {
                continue;
            }
            bodies.push(body);
            let shape = self.shape_at(body);
            for slot in (0..shape.slots()).map(|at| Slot(at as u32)) {
                if !matches!(shape.slot_name(slot), BinderSymbol::Value(_)) {
                    continue;
                }
                let unit = self.shapes.slot_unit(body, slot);
                let member = (self.store.fact(unit))
                    .and_then(|(facts, _)| facts.typed.get(body))
                    .and_then(|(_, typed)| typed.contents.get(slot))
                    .map(|(_, tree)| *tree);
                pending.extend(member);
            }
        }
        held.sort_unstable();
        collect(self.lay, held.iter().copied())
    }

    /// Each holder in `segment` holding a module of `identity`'s tree, binders in slot order then
    /// views in site order, beside its tree and the newest unit that fixed it.
    fn holding_at(
        &self,
        segment: Segment,
        identity: Identity,
    ) -> BumpVec<'p, (Holder, Tag, Tree<'o>)> {
        let mut holders = BumpVec::new_in(self.scratch);
        let mut at = self.holders.get(&(segment, identity)).copied();
        while let Some(index) = at {
            let (holder, before) = self.held[index as usize];
            holders.push(holder);
            at = before;
        }
        holders.sort_unstable();
        holders.dedup();
        let mut found = BumpVec::with_capacity_in(holders.len(), self.scratch);
        for holder in holders {
            let (tree, tag, identities) = self.holding[&(segment, holder)];
            // A binder bound again since holds what its newest content holds.
            if identities.contains(&identity) {
                found.push((holder, tag, tree));
            }
        }
        found
    }

    /// The unit running, whose record keeps what the walk fixes now: none while an unfilled
    /// typing fixes nothing.
    fn fixing(&mut self) -> Option<&mut UnitState<'p, 'o>> {
        if self.unfilled {
            return None;
        }
        let tag = self.owner_of(*self.open.last()?);
        Some(&mut self.units[tag.0 as usize])
    }

    /// The unit whose record keeps what `tag` fixes: its entry unit for a seed, else itself.
    fn owner_of(&self, tag: Tag) -> Tag {
        self.units[tag.0 as usize].owner.unwrap_or(tag)
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
    /// name of the module its operand is typed, and an arm's `it` at the interval its `MATCH` typed
    /// it at, as `entering` says.
    fn entry(&mut self, level: usize, entering: Entering) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        for slot in shape.holds(UnitIndex::ENTRY).slots.iter().copied() {
            if self.unfilled {
                self.seed_entry_slot(level, slot, entering)?;
                continue;
            }
            // Each binder is seeded in a gathering of its own, which a read of it gathers alone.
            let entry = *self.open.last().expect("the entry unit runs");
            let id = self.units[entry.0 as usize].id;
            let seed = self.open_unit(id, true);
            self.units[seed.0 as usize].owner = Some(entry);
            self.seed_entry_slot(level, slot, entering)?;
            let frontier = self.end(seed);
            // The paths the seed resolved rest in the entry unit's record, which lays its seeds.
            let paths = std::mem::replace(
                &mut self.units[seed.0 as usize].paths,
                BumpVec::new_in(self.scratch),
            );
            self.units[entry.0 as usize].paths.extend(paths);
            self.units[entry.0 as usize].seeds.push((slot, frontier));
            self.gather(seed);
        }
        Ok(())
    }

    /// Seed the binder at `slot`, one of the entry unit's of the shape at `level`.
    fn seed_entry_slot(
        &mut self,
        level: usize,
        slot: Slot,
        entering: Entering,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        // A surfaced name reads the module's static type, which its `USING` read, and an arm's
        // `it` the interval its `MATCH` typed it at.
        let entered = match shape.position_of(slot) == Position::PARAMETER {
            true => entering,
            false => Entering::Plain,
        };
        if let Entering::Surfaced(_, tag) | Entering::Arm(_, tag) = entered {
            self.gather(tag);
        }
        // A block's surfaced key is a registration bound at its entry.
        if let Some(registration) = shape.registration(slot) {
            self.register(level, registration)?;
        }
        self.seed_slot(level, slot, entered)
    }

    /// Seed the binder at `slot` of the shape at `level` before its unit types anything: a
    /// parameter the run retypes by its fit at the paths through it
    /// ([`at_its_paths`](Self::at_its_paths)), once it holds its seed for them to read.
    fn seed_slot(
        &mut self,
        level: usize,
        slot: Slot,
        entering: Entering,
    ) -> Result<(), ShapeError<'o>> {
        let seeded = self.seeded(level, slot, entering);
        self.set_binder(level, slot, seeded);
        let mut own = seeded;
        if let (DeclaredType::Type(typed), Some(declared)) =
            (seeded, self.retyped_parameter(level, slot, entering))
        {
            let binder = Binder {
                body: self.chain[level].shape.segment(),
                slot,
            };
            let pinned = self.at_its_paths(level, Root::Binder(binder), &[], declared, typed)?;
            // A `:Module` parameter is exactly the type its argument carries, which no other node
            // spells: the name a binding of it gives that type.
            let pinned = match (self.chain[level].shape.slot_name(slot), declared) {
                (BinderSymbol::Value(name), declared)
                    if pinned.lower != pinned.upper
                        && declared == Parametric::from(KType::EMPTY_SIGNATURE) =>
                {
                    let term = crate::scope::DerivedTerm::Carried {
                        binder: self.unknown_at(level, slot),
                        name,
                    };
                    self.derive(term).map_or(pinned, Interval::point)
                }
                _ => pinned,
            };
            if pinned != typed {
                own = DeclaredType::Type(pinned);
                self.set_binder(level, slot, own);
            }
        }
        // The seed a binding of the binder writes into: pinned at its paths where it is.
        self.note_own(level, slot, own, None);
        Ok(())
    }

    /// The type the run retypes the parameter at `slot` of the shape at `level` to: a callable's
    /// declared type for its argument, and the upper end of an arm's `it` under a type or label
    /// guard, its scrutinee retyped to the guard. `None` for any other binder, and for one a
    /// binding or a surfacing seeds ([`seeded`](Self::seeded)).
    fn retyped_parameter(
        &self,
        level: usize,
        slot: Slot,
        entering: Entering,
    ) -> Option<Parametric> {
        let shape = self.chain[level].shape;
        let name = shape.slot_name(slot);
        if shape.position_of(slot) != Position::PARAMETER
            || !matches!(name, BinderSymbol::Value(_))
            || shape.registration(slot).is_some()
            || self.binds(self.unknown_at(level, slot)).is_some()
        {
            return None;
        }
        let guard = shape.arm().map(|arm| arm.guard);
        match entering {
            Entering::Plain if shape.kind() == ShapeKind::Callable => {
                self.in_body(level, Part::Parameter(name.symbol()))
            }
            Entering::Arm(it, _) if matches!(guard, Some(Guard::Type(_) | Guard::Label(_))) => {
                Some(it.upper)
            }
            _ => None,
        }
    }

    /// Run the unit `index` of the shape at `level`, doing `work`, in its five steps: the paths
    /// its statements spell, its declarations, its type expressions and registrations and the
    /// callables it births, its value statics, and its binder facts beside them. `nests` is its
    /// statements and what they nest.
    fn steps(
        &mut self,
        level: usize,
        index: UnitIndex,
        work: UnitWork,
        nests: UnitStatements<'p, 'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let holds = shape.holds(index);
        for slot in holds.slots.iter().copied() {
            if !self.component_types(shape, slot) && shape.declarations(slot).is_none() {
                self.seed_slot(level, slot, Entering::Plain)?;
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
                    self.seed_slot(level, *member, Entering::Plain)?;
                }
            }
        }
        // 3. Each type expression, in program order, then the guards; each registration it binds;
        //    each callable it births.
        self.type_expressions(level, holds.expressions)?;
        for slot in holds.slots.iter().copied() {
            if let Some(registration) = shape.registration(slot) {
                self.register(level, registration)?;
                self.seed_slot(level, slot, Entering::Plain)?;
            }
        }
        for nested in nests.callables {
            self.birth(level, nested)?;
        }
        // 4 and 5. Each quote's code it births, walked first so an `EVAL` reads it typed; then its
        //    value statics, each binder's fact beside them.
        for nested in nests.codes {
            self.body(Some(level), nested)?;
            // The unit stands for its code's typing: reading the code's closing record gathers its
            // frontier, so a binding that wakes the code re-runs the unit, which walks it again.
            let _ = self.closed(nested.segment());
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
        // A block its value statics did not reach — a block a builtin's rule reads no further than
        // its value — is typed on its own, inside the unit.
        for nested in nests.blocks {
            if !self.walked.contains(&nested.segment()) {
                self.block(level, nested, Entering::Plain)?;
            }
        }
        // A statement no rule typed is at most `Any`.
        for index in nests.statements {
            if self.chain[level].statements[*index].is_none() {
                self.set_statement(level, *index, unknown());
            }
        }
        if !self.unfilled {
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
                (Some(rhs), _) => self.site(level, rhs),
                (None, Some(born)) => born,
                (None, None) => continue,
            };
            let tree = self.slot_tree(trees::Scope::Chain(level), slot, site)?;
            self.record_content(level, slot, tree);
        }
        Ok(())
    }

    /// Type the inline body `nested` — an arm, a block, a `USING … SCOPE` body, a `MODULE` or
    /// `GROUP` body, entered as `entering` says — inside the unit running it, nested in the shape at
    /// `level`: its entry and its units, each in a record of its own, each callable a unit births
    /// walked right after it. The running unit reads each of their frontiers, cut at it, so it
    /// wakes wherever one of them would on a name bound outside `nested`, and keeps the body's
    /// signature. What `finish` makes of it.
    fn inline<T>(
        &mut self,
        level: usize,
        nested: &'graph BodyShape<'graph>,
        entering: Entering,
        finish: impl FnOnce(&mut Self, usize) -> Result<T, ShapeError<'o>>,
    ) -> Result<(T, Level<'p, 'o, 'graph>), ShapeError<'o>> {
        self.push(nested, false, None);
        let at = level + 1;
        let finished = match self.units_of(at, entering) {
            Ok(()) => finish(self, at),
            Err(error) => Err(error),
        };
        if finished.is_ok() && !self.unfilled {
            let units =
                (nested.units().iter().enumerate()).map(|(index, _)| UnitIndex::of_run(index));
            for unit in [UnitIndex::ENTRY].into_iter().chain(units) {
                let id = UnitId {
                    segment: nested.segment(),
                    unit,
                };
                if let Some(tag) = self.started.get(&id) {
                    self.gather(*tag);
                }
            }
            if let Some(signature) = self.chain[at].signature {
                self.keep_signature(nested.segment(), signature);
            }
        }
        let popped = self.chain.pop().expect("the body was pushed");
        self.leave(nested);
        self.walked.insert(nested.segment());
        finished.map(|finished| (finished, popped))
    }

    /// Type the block `nested` — an arm, or a `USING … SCOPE` body, entered as `entering` says —
    /// nested in the shape at `level`, folding its class in: its last statement's static type.
    fn block(
        &mut self,
        level: usize,
        nested: &'graph BodyShape<'graph>,
        entering: Entering,
    ) -> Result<Option<Interval>, ShapeError<'o>> {
        let last = |pass: &mut Self, at: usize| Ok(pass.last_statement(at));
        let (last, popped) = self.inline(level, nested, entering, last)?;
        self.fold(level, &popped);
        Ok(last)
    }

    /// Keep `signature`, the exact signature of the `MODULE` or `GROUP` body `segment` just typed,
    /// in the unit running it: the newest typing of a body the unit types twice stands.
    fn keep_signature(&mut self, segment: Segment, signature: Parametric) {
        let tag = self.innermost();
        let held = &mut self.units[tag.0 as usize].typed;
        held.retain(|(each, _)| *each != segment);
        held.push((
            segment,
            Typed {
                signature: Some(signature),
                ..Typed::EMPTY
            },
        ));
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
        let (typed, popped) = self.inline(level, body, Entering::Plain, signed)?;
        self.fold(level, &popped);
        let mints = !popped.minting.is_empty() || popped.minted_within;
        self.module_mints.insert(address(body), mints);
        Ok(typed)
    }

    /// The signature of the `MODULE` or `GROUP` body at `level`, written over names, beside its
    /// static type: the point at it where each value member is one type, else, on whether each
    /// inexact member's ends are one, that point or at most the empty signature. The signature is
    /// kept where every member is one type at every binding. At most the empty signature where a
    /// member is unknown before the run.
    fn signed(
        &mut self,
        level: usize,
        body: &'graph BodyShape<'graph>,
    ) -> Result<(Interval, Option<Parametric>), ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let unknown = Ok((under(KType::EMPTY_SIGNATURE.into()), None));
        let mut members = BumpVec::with_capacity_in(body.slots(), scratch);
        let mut keyworded = BumpVec::new_in(scratch);
        let mut inexact = BumpVec::new_in(scratch);
        for index in 0..body.slots() {
            let slot = Slot(index as u32);
            let (bound, _) = self.chain[level].binders[index];
            let member = match body.slot_name(slot) {
                BinderSymbol::Value(_) => match bound {
                    DeclaredType::Type(typed) => {
                        if typed.lower != typed.upper {
                            inexact.push(Question::Same {
                                a: typed.lower.into(),
                                b: typed.upper.into(),
                            });
                        }
                        DeclaredType::Type(typed.upper)
                    }
                    // A member's scheme, closed or written over names.
                    DeclaredType::Scheme(scheme) => {
                        let Some(born) = body.births(slot) else {
                            return unknown;
                        };
                        let typed = self.callable_cell(born);
                        if let Static::Unknown = self.resolved_callable(typed) {
                            return unknown;
                        }
                        DeclaredType::Scheme(scheme)
                    }
                },
                BinderSymbol::Type(_) => match self.declared_cell(body, slot) {
                    Static::Closed(held) => DeclaredType::Type(held.into()),
                    Static::Rigid { value, .. } => DeclaredType::Type(value),
                    Static::Unknown => return unknown,
                },
                BinderSymbol::Registration(_) => {
                    match self.resolved_registered(self.registered_cell(body, slot)) {
                        Static::Closed(registered) => keyworded.push(registered.shape.into()),
                        Static::Rigid { value, .. } => keyworded.push(value.shape),
                        Static::Unknown => return unknown,
                    }
                    continue;
                }
                BinderSymbol::Key(_) => unreachable!("no binder declares a key"),
            };
            members.push((slot, member));
        }
        let (signed, operands) = module_signing(body, members, keyworded, scratch);
        let Some(signature) = judge::signature(self, types, scratch, &signed, operands) else {
            return unknown;
        };
        let lower = judge::when_all(
            self,
            types,
            scratch,
            &inexact,
            &mut || signature,
            KType::NEVER.into(),
        );
        let upper = judge::when_all(
            self,
            types,
            scratch,
            &inexact,
            &mut || signature,
            KType::EMPTY_SIGNATURE.into(),
        );
        let kept = (lower == signature).then_some(signature);
        Ok((Interval { lower, upper }, kept))
    }

    /// Fold `inner`, a block or module body the shape at `level` runs, into it: its exits and
    /// its run's minting sites count toward the frame body enclosing it.
    fn fold(&mut self, level: usize, inner: &Level<'p, 'o, 'graph>) {
        let at = &mut self.chain[level];
        at.exits |= inner.exits;
        at.folded += inner.run_mints();
        at.minted_within |= inner.minted_within || !inner.minting.is_empty();
        // The unit running `inner` is the shape's own, which a resumption absorbing it folds back.
        if let Some(unit) = self.fixing() {
            unit.exits |= inner.exits;
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
        let types = self.types;
        if judge::is(self, types, last.upper, KType::NEVER) {
            return Ok(());
        }
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
                Written::Node(self.node_site(level, node)),
                last,
            );
        };
        if let Some(body) = shape.births(slot)
            && body.kind() == ShapeKind::Module
        {
            let scope = trees::Scope::Chain(level);
            let tree = self.slot_tree(scope, slot, self.node_site(level, node))?;
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
            overlap: None,
            retypes: self.retypes(level),
        };
        Ok((closing, tag))
    }

    /// What a frame of the callable body at `level` re-stamps each argument and its value at, where
    /// the walk closes the declared type of each.
    fn retypes(&self, level: usize) -> Retypes<'o> {
        let (shape, types) = (self.chain[level].shape, self.types);
        if shape.kind() != ShapeKind::Callable {
            return Retypes::NONE;
        }
        let closed = |part| {
            let declared = self.in_body(level, part)?;
            judge::restamp(self, types, self.scratch, declared)
        };
        let mut parameters = BumpVec::new_in(self.scratch);
        parameters.extend((0..shape.slots() as u32).map(Slot).filter_map(|slot| {
            if shape.position_of(slot) != Position::PARAMETER {
                return None;
            }
            let BinderSymbol::Value(name) = shape.slot_name(slot) else {
                return None;
            };
            Some((name.symbol(), closed(Part::Parameter(name.symbol()))?))
        }));
        Retypes {
            parameters: collect(self.lay, parameters.iter().copied()),
            returns: closed(Part::Return),
        }
    }

    /// The quote's code `code`'s last statement's static type as a traced `EVAL` runs it: typed a
    /// second time, on the chain around it, with every unmarked hole empty. It fixes nothing, and
    /// a use it can never run refuses nothing: its `EVAL` faults.
    fn ran(&mut self, code: &'graph BodyShape<'graph>) -> Option<Interval> {
        let unfilled = std::mem::replace(&mut self.unfilled, true);
        let (depth, open) = (self.chain.len(), self.open.len());
        self.push(code, true, None);
        let level = self.chain.len() - 1;
        self.open_paths(code);
        let walked = self.units_of(level, Entering::Plain);
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
        demand.marks.insert(
            segment,
            Mark {
                index: demand.next,
                low: demand.next,
                on_stack: true,
                self_edge: false,
            },
        );
        demand.stack.push(segment);
        demand.next += 1;
    }

    /// Finish the callable `segment`, its closing computed: written at once where it heads its
    /// component, else kept until its component is finished. A component of more than one body,
    /// or of one that calls itself, is recursive: none of its bodies keeps a summary.
    fn finish(&mut self, segment: Segment, closing: Closing<'o>, frontier: Frontier<'o>) {
        self.demand.pending.push((segment, closing, frontier));
        let mark = self.demand.marks[&segment];
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
            if let Some(mark) = self.demand.marks.get_mut(member) {
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
            pass.demand
                .marks
                .get(&segment)
                .is_some_and(|mark| mark.on_stack)
        };
        let reaches = |pass: &mut Self, low: u32| {
            if let Some(caller) = pass.caller()
                && let Some(mark) = pass.demand.marks.get_mut(&caller)
            {
                mark.low = mark.low.min(low);
                if caller == segment {
                    mark.self_edge = true;
                }
            }
        };
        if on_stack(self) {
            let index = self.demand.marks[&segment].index;
            reaches(self, index);
            return Ok(false);
        }
        if !self.walked.contains(&segment) {
            if self.unfilled || !self.typed_on_demand(callee)? {
                return Ok(false);
            }
            if on_stack(self) {
                let low = self.demand.marks[&segment].low;
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
        self.note_frontier(frontier, segment);
        Some(self.standing_closing(segment, closing, frontier))
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
            if self.shapes.home(shape.segment(), variable).is_some() {
                return;
            }
            if matches!(
                shape.kind(),
                ShapeKind::Callable | ShapeKind::Module | ShapeKind::Code
            ) {
                self.composed.insert((shape.segment(), variable));
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
    /// where it is closed; and `upper` beside where each lexical variable it names is read. `None`
    /// where the load reads nothing of `upper`, at every binding alike: the call reads the carried
    /// type there too.
    fn contribution(
        &mut self,
        level: usize,
        upper: Parametric,
        at: SourceRef,
    ) -> Option<StaticType<'o>> {
        if judge::is(self, self.types, upper, KType::ANY) {
            return Some(Static::Unknown);
        }
        let variables = self.located(level, &[upper], at);
        if variables.is_empty() {
            return judge::static_of(self.types, upper).map(Static::Closed);
        }
        Some(Static::Rigid {
            value: upper,
            variables,
        })
    }

    /// The solution `solution` of an instance site in the shape at `level`, as its cell records it:
    /// closed where every entry is concrete, and otherwise beside where the site reads each lexical
    /// variable an entry names.
    /// The point each variable of `judgement`'s group solves to at every run of the call at `at`
    /// in the shape at `level`, whose arguments contribute `contributions`: closed where its
    /// interval is exactly a closed type, rigid where it is exactly one over the run's lexical
    /// variables — which the layer binding them closes before any run reads it — and the carried
    /// type where the variable is the whole type of one slot, which [`judge_by_class`] admits as
    /// *always* at the variable's bound, and that slot's argument contributes nothing. `None` where
    /// a variable has none of the three.
    fn points(
        &mut self,
        level: usize,
        judgement: &Judgement<'_>,
        contributions: &[StaticType<'o>],
        at: SourceRef,
    ) -> Option<Points<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let Some(DeclaredType::Scheme(scheme)) = judgement.known.shape() else {
            return Some(Points::NONE);
        };
        let intervals = judgement.intervals?;
        let mut slots = BumpVec::new_in(scratch);
        slots.extend(scheme_slots(scheme, types));
        let mut points = BumpVec::with_capacity_in(intervals.len(), scratch);
        let mut rigid = BumpVec::new_in(scratch);
        for (variable, interval) in intervals.iter().enumerate() {
            // An exact point is closed or rigid, which a binding closes alike.
            let exact = judge::exact(self, types, *interval);
            points.push(
                match exact.then(|| judge::static_of(types, interval.upper)) {
                    Some(Some(point)) => Point::Closed(point),
                    Some(None) => {
                        rigid.push(interval.upper);
                        Point::Rigid(interval.upper)
                    }
                    None => {
                        // No binding turns a lexical variable or a projection into the
                        // scheme's own variable, so the match is bookkeeping.
                        let bare = |slot: &Parametric| {
                            matches!(judge::own_variable(types, *slot),
                            TypeNode::Quantified { index, .. } if index == variable)
                        };
                        let mut bares = (0..slots.len()).filter(|slot| bare(&slots[*slot]));
                        let (Some(slot), None) = (bares.next(), bares.next()) else {
                            return None;
                        };
                        if contributions
                            .get(slot)
                            .is_some_and(|contributed| !matches!(contributed, Static::Unknown))
                        {
                            return None;
                        }
                        Point::Carried(u16::try_from(slot).ok()?)
                    }
                },
            );
        }
        let variables = match rigid.is_empty() {
            true => &[][..],
            false => self.located(level, &rigid, at),
        };
        Some(Points {
            points: collect(self.lay, points.iter().copied()),
            variables,
        })
    }

    /// The instance made at `solution` of type `ktype`, at `at` in the shape at `level`: closed
    /// where both are, and otherwise rigid over the lexical variables they name, located once.
    fn instance(
        &mut self,
        level: usize,
        solution: &[Parametric],
        ktype: Parametric,
        at: SourceRef,
    ) -> StaticInstance<'o> {
        let types = self.types;
        let mut closed = BumpVec::with_capacity_in(solution.len(), self.scratch);
        closed.extend(
            solution
                .iter()
                .map_while(|each| judge::static_of(types, *each)),
        );
        if closed.len() == solution.len()
            && let Some(ktype) = judge::static_of(types, ktype)
        {
            return StaticInstance::Closed {
                solution: collect(self.lay, closed.iter().copied()),
                ktype,
            };
        }
        let mut named = BumpVec::with_capacity_in(solution.len() + 1, self.scratch);
        named.extend(solution.iter().copied());
        named.push(ktype);
        let variables = self.located(level, &named, at);
        assert!(
            !variables.is_empty(),
            "an instance names no variable but its chain's lexical ones"
        );
        StaticInstance::Rigid {
            solution: collect(self.lay, solution.iter().copied()),
            ktype,
            variables,
        }
    }

    /// Where the shape at `level` reads each lexical variable `values` name, by level: a hop per
    /// block and a type capture per callable or module between the read and the variable's home,
    /// each added for the use at `used`, and a path variable through the root of a path of its
    /// class. Empty where they name none.
    fn located(&mut self, level: usize, values: &[Parametric], used: SourceRef) -> &'o [Located] {
        let (types, scratch) = (self.types, self.scratch);
        let (mut levels, mut classes): (BumpVec<'_, (usize, TypeSymbol)>, BumpVec<'_, PathClass>) =
            (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
        let mut variables = BumpVec::new_in(scratch);
        for value in values {
            judge::read_points(types, scratch, *value, Side::Above, &mut |variable| {
                variables.push(variable);
                None
            });
        }
        // A derived name reads what its term reads.
        let (mut at, mut terms) = (0, BumpVec::new_in(scratch));
        while let Some(variable) = variables.get(at).copied() {
            at += 1;
            match variable {
                Variable::Lexical { level, name, .. }
                    if !levels.iter().any(|(at, _)| *at == level) =>
                {
                    levels.push((level, name));
                }
                Variable::Path { class, .. } if !classes.contains(&class) => {
                    classes.push(class);
                }
                Variable::Derived { term, .. } if !terms.contains(&term) => {
                    terms.push(term);
                    if let Some(name) = self.store.derived_name(term) {
                        name.term.each_type(&mut |ty| {
                            variables
                                .extend(judge::free_variables(types, scratch, ty).iter().copied());
                        });
                    }
                }
                _ => {}
            }
        }
        let mut located = BumpVec::with_capacity_in(levels.len() + classes.len(), scratch);
        for (variable, name) in levels.iter().copied() {
            let at = self.coordinate_of(level, variable, (name, used));
            located.push(Located::Type {
                level: variable,
                at,
            });
        }
        // A path variable is read through a path of its class whose root this shape reaches, the
        // application it is read through located beside it.
        for class in classes.iter().copied() {
            let witness = (self.reachable(level, class))
                .expect("a path variable a static type names has a root its shape reaches");
            let entry = self.witnessed(level, witness);
            located.push(Located::Path {
                class,
                at: witness.coordinate,
                root: self.chain_root_segment(level),
                entry: entry.name,
            });
        }
        collect(self.lay, located.iter().copied())
    }

    /// A slot's static type before any unit runs: a registration's function type, a type name's
    /// type value's, a parameter's declared type as its body reads it, a name a `USING … SCOPE`
    /// body surfaces the member read it names of its operand ([`member_read`]), an arm's `it` the
    /// interval its `MATCH` typed it at, and `[Never, Any]` for a local its unit sets.
    fn seeded(&self, level: usize, slot: Slot, entering: Entering) -> Bound {
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
        if let (Entering::Arm(it, _), BinderSymbol::Value(_)) = (entering, shape.slot_name(slot)) {
            return DeclaredType::Type(it);
        }
        if let (Entering::Surfaced(surfacing, _), BinderSymbol::Value(_)) =
            (entering, shape.slot_name(slot))
            && shape.position_of(slot) == Position::PARAMETER
        {
            let name = shape.slot_name(slot);
            return match signature_member(self, self.types, self.scratch, surfacing.upper, name) {
                Some(SignatureMember {
                    declared: DeclaredType::Type(declared),
                    unpinned: None,
                }) => DeclaredType::Type(member_read(
                    self,
                    self.types,
                    self.scratch,
                    surfacing,
                    declared,
                )),
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
                .map_or(DeclaredType::Type(unknown()), |body| self.callable(body));
        }
        let name = shape.slot_name(slot);
        if let BinderSymbol::Type(symbol) = name {
            return DeclaredType::Type(match self.declared_cell(shape, slot) {
                Static::Closed(handle) if shape.declarations(slot).is_some() => {
                    Interval::point(KType::of_kind(handle.kind_of(self.types)).into())
                }
                _ => match self.shapes.level(shape.segment(), Address::Slot(slot)) {
                    Some(at) => {
                        let variable = self.types.lexical(at, symbol, KType::ANY);
                        Interval::point(judge::kind_value(self, self.types, self.scratch, variable))
                    }
                    None => under(KType::ANY_TYPE.into()),
                },
            });
        }
        let is_parameter = shape.position_of(slot) == Position::PARAMETER;
        if shape.kind() != ShapeKind::Callable || !is_parameter {
            return DeclaredType::Type(unknown());
        }
        let declared = self.in_body(level, Part::Parameter(name.symbol()));
        // Declared where the callable is written, a path there may not be reachable here.
        let typed = judge::retyped(
            self,
            self.types,
            self.scratch,
            declared.unwrap_or_else(|| KType::ANY.into()),
        );
        DeclaredType::Type(self.sourced(level, typed))
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
                self.record_minting(level, self.node_site(level, first));
                self.record_nominal(level, self.node_site(level, first), component.members);
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
            let mut typed = match self.callable(body) {
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
                Some(declared) => judge::retyped(self, self.types, self.scratch, declared),
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
            // A member's content tree, which its unit fixes once every member is typed.
            let tree = match (
                shape.rhs(*member),
                self.module_unknown(level, *member, DeclaredType::Type(value)),
            ) {
                (Some(rhs), Some(_)) if !self.unfilled => {
                    let site = self.site(level, rhs);
                    Some(self.slot_tree(trees::Scope::Chain(level), *member, site)?)
                }
                _ => None,
            };
            self.note_own(level, *member, DeclaredType::Type(value), tree);
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
        if index + 1 != shape.body().len() {
            return None;
        }
        // A tail arm's last statement is its `MATCH`'s, the last of the body around it.
        if shape.arm().is_some_and(|arm| arm.tail) {
            let around = self.chain[level - 1].shape.body().len();
            return self.tail(level - 1, around - 1);
        }
        if shape.kind() != ShapeKind::Callable {
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
            let solution = self.instance(level, solution, typed.upper, statement.source);
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
            wanted.filter(|wanted| judge::is_function(self, types, scratch, *wanted))
        else {
            return Err(self.unfixed(scheme, wanted, at));
        };
        let record = judge::instance(self, types, scratch, scheme, wanted);
        let Some(solved) = record.solved else {
            return Err(ShapeError::NoInstance { scheme, wanted, at });
        };
        if record.reached.contains(&false) {
            let mut unfixed = BumpVec::new_in(scratch);
            unfixed.extend((0..quantifiers.len()).filter(|index| !record.reached[*index]));
            return Err(ShapeError::Unfixed {
                variables: collect(self.lay, unfixed.iter().map(|index| quantifiers[*index])),
                wanted: Some(wanted),
                at,
            });
        }
        let solution = solved.types;
        let instance = instantiate_quantified(types, scratch, scheme, solution);
        Ok((Interval::point(instance), solution))
    }

    /// Where the part at `site` of the shape at `level` is written.
    fn source(&self, level: usize, site: Site) -> SourceRef {
        let statement = self.statement_holding(level, site);
        let shape = self.chain[level].shape;
        source_of(shape, &shape.body()[statement], site)
            .expect("a part lies in its shape's statements")
    }

    /// The site of `part` in the shape at `level`.
    pub(super) fn site(&self, level: usize, part: &ExpressionPart<'_>) -> Site {
        self.chain[level].shape.site(part)
    }

    /// The site of `node` in the shape at `level`.
    pub(super) fn node_site(&self, level: usize, node: &KExpression<'_>) -> Site {
        self.chain[level].shape.site_of_node(node)
    }

    /// The statement of the shape at `level` holding the part at `site`: the one the walk is
    /// typing, where it holds the part, else the one a scan of the body finds.
    pub(super) fn statement_holding(&self, level: usize, site: Site) -> usize {
        let at = &self.chain[level];
        let body = at.shape.body();
        if body
            .get(at.statement)
            .is_some_and(|node| part_of(at.shape, node, site).is_some())
        {
            return at.statement;
        }
        (body
            .iter()
            .position(|node| part_of(at.shape, node, site).is_some()))
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
                let Some(mention) = shape.mention(shape.site(leaf)) else {
                    return Ok(None);
                };
                match self.read_declared(level, mention.coordinate) {
                    DeclaredType::Scheme(scheme) => Some((
                        Instanced::Name(leaf),
                        scheme,
                        self.source(level, self.site(level, leaf)),
                    )),
                    DeclaredType::Type(_) => None,
                }
            }
            Form::Lambda(node) => {
                let Some(body) = shape.site_of_body(node).and_then(|site| shape.nested(site))
                else {
                    return Ok(None);
                };
                match self.callable(body) {
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
            } => {
                let peeked = self.peek(level, operand);
                match self.peeked_member_scheme(level, operand, peeked, label)? {
                    Some((scheme, None)) => Some((Instanced::Member(node), scheme, node.source)),
                    _ => None,
                }
            }
            _ => None,
        })
    }

    /// [`member_scheme`](Self::member_scheme) where the operand is [peeked](Self::peeked) as
    /// `peeked`: `None` for any other.
    fn peeked_member_scheme(
        &mut self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        peeked: Option<Interval>,
        name: BinderSymbol,
    ) -> Result<Option<(Scheme, Option<BinderSymbol>)>, ShapeError<'o>> {
        match peeked {
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
            let solution = self.instance(level, solution, typed.upper, node.source);
            self.record_instance_read(
                level,
                self.node_site(level, node),
                InstanceRead::Solved(solution),
            );
        }
        Ok(typed)
    }

    /// [`peeked`](Self::peeked) through the level's memo: each part is peeked once.
    fn peek(&mut self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Interval> {
        let (site, gathering) = (self.site(level, part), self.open.last().copied());
        if let Some((peeked, noted)) = self.chain[level].peeks.get(&site) {
            debug_assert_eq!(
                *noted, gathering,
                "a hit notes into the gathering its miss did"
            );
            return *peeked;
        }
        let peeked = self.peeked(level, part);
        self.chain[level].peeks.insert(site, (peeked, gathering));
        peeked
    }

    /// The static type of `part`, a name or a chain of member reads rooted at one, read without
    /// typing it: a name's binder's, and a member read's as its rule gives it. `None` for any other
    /// part. Read through [`peek`](Self::peek), which memoizes it.
    fn peeked(&mut self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Interval> {
        #[cfg(test)]
        count(|counts| counts.peeks += 1);
        let shape = self.chain[level].shape;
        match of_part(shape, part) {
            Form::Name(leaf) => {
                let mention = shape.mention(shape.site(leaf))?;
                self.read_declared(level, mention.coordinate).as_type()
            }
            Form::Member { operand, label, .. } => {
                let operand = self.peek(level, operand)?;
                match signature_member(self, self.types, self.scratch, operand.upper, label)? {
                    SignatureMember {
                        declared: DeclaredType::Type(declared),
                        unpinned: None,
                    } => Some(member_read(
                        self,
                        self.types,
                        self.scratch,
                        operand,
                        declared,
                    )),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The type a quantified function argument at the slot `slot` of a callee whose group is
    /// bounded by `bounds` is wanted at: the group solved from the `others` first — each other slot
    /// beside the type the solve reads there ([`solved_from`](Self::solved_from)) — then `slot`
    /// read from above through it. A variable an earlier class than the slot's solves is taken as
    /// `fixed` gives it, which is empty where none is. Any other variable the slot shares with the
    /// others comes out one point the call reproduces: a closed one, one of a reproducible solve
    /// ([`SolveRecord::reproducible`](crate::type_lattice::SolveRecord::reproducible)), or, over
    /// exact arguments, the derived name a solve stored whole types ([`judge::stored_solve`]). One
    /// only the slot names is read through `[Never, bound]`. Refused where the others admit no
    /// solve at any binding, or leave a shared variable open. Each of the `others` is its slot's
    /// position beside the slot and the type the solve reads there.
    fn slot_wanted(
        &self,
        (scheme, bounds): (Scheme, &[KType]),
        slot: Parametric,
        others: &[(usize, Parametric, Interval)],
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
        let mut pinned = BumpVec::with_capacity_in(bounds.len(), scratch);
        pinned.extend((0..bounds.len()).map(fixed));
        let mut declared = BumpVec::with_capacity_in(others.len(), scratch);
        let mut admitted = BumpVec::with_capacity_in(others.len(), scratch);
        let mut involved = BumpVec::with_capacity_in(others.len(), scratch);
        let mut carried = BumpVec::with_capacity_in(others.len(), scratch);
        let mut exact = true;
        for (position, other, argument) in others.iter() {
            // One naming only fixed variables is admitted against them by the call, and says
            // nothing of the rest.
            if !(0..bounds.len())
                .any(|variable| names(*other, variable) && fixed(variable).is_none())
            {
                continue;
            }
            exact &= judge::exact(self, types, *argument);
            declared.push(*other);
            admitted.push((*other, *argument));
            involved.push((*other, argument.upper));
            carried.push((*position, argument.upper));
        }
        let record = judge::class_step(types, scratch, bounds, &pinned, &involved, exact);
        // Over exact arguments, a solve a run may solve otherwise is stored whole: whether it
        // solves is the binding's question, and each variable the slot names is the derived name
        // its solution types.
        let storable = bounds.len() < 64 && carried.iter().all(|(position, _)| *position < 64);
        if exact && !record.reproducible && storable {
            let stored =
                judge::stored_solve(self, types, scratch, scheme, &pinned, &carried, exact);
            let solved = stored.ok_or(Unsolved::Misfit)?;
            let mut chosen = BumpVec::with_capacity_in(bounds.len(), scratch);
            for (variable, bound) in bounds.iter().enumerate() {
                chosen.push(match fixed(variable) {
                    Some(to) => Interval::point(to),
                    None if names(slot, variable) => Interval::point(solved[variable]),
                    None => Interval::within(*bound),
                });
            }
            return Ok(judge::read_points(
                types,
                scratch,
                slot,
                Side::Above,
                &mut |variable| match variable {
                    Variable::Quantified { index, .. } => chosen.get(index).copied(),
                    _ => None,
                },
            ));
        }
        if !record.reproducible {
            judge::unreproduced(self, types, &involved);
        }
        let Some(solved) = record.solved else {
            // Only an argument meeting its slot at `Never` never fits; an imprecise one fixes
            // nothing.
            let misfit = (admitted.iter()).any(|(other, argument)| {
                judge::never_satisfied(self, types, scratch, *argument, *other)
            });
            return Err(match misfit {
                true => Unsolved::Misfit,
                false => named(),
            });
        };
        let (reproducible, solved) = (record.reproducible, solved.intervals);
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
                && !(judge::exact(self, types, interval)
                    // A point over names is open: its solution a name a binding closes.
                    && (reproducible || judge::static_of(types, interval.upper).is_some()))
            {
                open.push(variable);
            }
            chosen.push(interval);
        }
        if !open.is_empty() {
            return Err(Unsolved::Open(open));
        }
        // Each of the group's own variables at its chosen point: a substitution, which commutes.
        Ok(judge::read_points(
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
    /// solving slots, `solving`, at their contributions the load knows among `contributions`,
    /// pinned to what the classes before it solved. A variable is fixed only where that solve is
    /// reproducible ([`SolveRecord::reproducible`](crate::type_lattice::SolveRecord::reproducible)): the call binds each lexical variable as the load
    /// did. `None` for every other variable, and empty where there is none. `Misfit` where those
    /// contributions never admit at any binding, since the call then never runs; `Open` naming a
    /// variable a slot that solves it reads no contribution at — an instance argument's, or
    /// unknown — or one a run may solve otherwise.
    fn solved_earlier(
        &self,
        shape: DeclaredType<Parametric>,
        solving: &[bool],
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
            let mut involved = BumpVec::new_in(scratch);
            let mut carried = BumpVec::new_in(scratch);
            let mut unknown = None;
            for slot in 0..slots.len() {
                if !(class(slot) == current
                    && solving[slot]
                    && shared.iter().any(|v| names(slot, *v)))
                {
                    continue;
                }
                let contribution = match contributions[slot] {
                    Static::Closed(contribution) => contribution.into(),
                    Static::Rigid { value, .. } => value,
                    Static::Unknown => {
                        unknown = Some(slot);
                        break;
                    }
                };
                involved.push((slots[slot], contribution));
                carried.push((slot, contribution));
            }
            if involved.is_empty() && unknown.is_none() {
                continue;
            }
            // The contributions known before an unknown one are still admitted: a failure there
            // that a run reproduces never admits at any binding.
            let record = judge::class_step(types, scratch, bounds, &fixed, &involved, false);
            let storable = bounds.len() < 64 && carried.iter().all(|(slot, _)| *slot < 64);
            if !record.reproducible && (unknown.is_some() || !storable) {
                judge::unreproduced(self, types, &involved);
            }
            let solution = match (record.solved, record.reproducible, unknown) {
                (None, true, _) => return Err(Unsolved::Misfit),
                (_, _, Some(slot)) => return Err(open(slot)),
                (Some(solved), true, None) => solved.types,
                // A solve a run may solve otherwise is stored whole.
                (_, false, None) if storable => {
                    judge::stored_solve(self, types, scratch, scheme, &fixed, &carried, false)
                        .ok_or(Unsolved::Misfit)?
                }
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

    /// The slots `shape`'s solve reads: [`solving_slots`], which every walk read of them goes
    /// through.
    fn solving(&self, shape: DeclaredType<Parametric>) -> &'p [bool] {
        #[cfg(test)]
        count(|counts| counts.solving_reads += 1);
        solving_slots(self.types, self.scratch, shape)
    }

    /// What a shape's solve reads at each slot, where `solving` is the slots it solves from
    /// ([`solving`](Self::solving)): a slot that solves, whose argument contributes, exactly that
    /// argument's upper end — the call solves from that type — and every other slot its argument's
    /// static type. `contributions` past its end are `Unknown`.
    fn solved_from(
        &self,
        solving: &[bool],
        arguments: &[Interval],
        contributions: &[StaticType<'o>],
    ) -> &'p [Interval] {
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
    /// registered shape is `shape`, solving from `solving`: wanted at its slot's type, read through
    /// the candidate's group solved from its other arguments where it has one, each as the solve
    /// reads it ([`solved_from`](Self::solved_from)) — a variable a class before the slot's solves
    /// taken from that class's solving slots at their contributions, as the call solves it, class
    /// by class. Refused where the load does not know the shape or makes no instance; `None` where
    /// the other arguments do not fit their slots, which the judge refuses.
    fn instances(
        &self,
        shape: Option<DeclaredType<Parametric>>,
        solving: &[bool],
        sites: &[InstanceArgument<'graph>],
        arguments: &[Interval],
        contributions: &[StaticType<'o>],
    ) -> Result<&'p [Made<'p>], Option<ShapeError<'o>>> {
        let (types, scratch) = (self.types, self.scratch);
        let mut slots = BumpVec::new_in(scratch);
        let Some(shape) = shape else {
            return Err(Some(self.unfixed(sites[0].scheme, None, sites[0].at)));
        };
        let (scheme, bounds, quantifiers) = match shape {
            DeclaredType::Type(shape) => {
                slots.extend(shape_slots(shape, types));
                (None, &[][..], &[][..])
            }
            DeclaredType::Scheme(scheme) => {
                slots.extend(scheme_slots(scheme, types));
                let group = scheme.group(types);
                (Some(scheme), group.bounds, group.names)
            }
        };
        let read = self.solved_from(solving, arguments, contributions);
        let mut others = BumpVec::with_capacity_in(slots.len(), scratch);
        for (position, slot) in slots.iter().enumerate() {
            if !sites.iter().any(|site| site.position == position) {
                others.push((position, *slot, read[position]));
            }
        }
        let mut made = BumpVec::with_capacity_in(sites.len(), scratch);
        for site in sites.iter() {
            let slot = slots[site.position];
            let wanted = if let (Some(scheme), false) = (scheme, bounds.is_empty()) {
                let fixed = self.solved_earlier(shape, solving, site.position, contributions);
                let wanted = |fixed| self.slot_wanted((scheme, bounds), slot, &others, fixed);
                match fixed.and_then(wanted) {
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
            } else {
                slot
            };
            made.push(self.instantiate(site.scheme, Some(wanted), site.at)?);
        }
        Ok(made.leak())
    }

    /// `Unfixed` for the quantified function typed by `scheme` at `at`, naming its whole group.
    fn unfixed(&self, scheme: Scheme, wanted: Option<Parametric>, at: SourceRef) -> ShapeError<'o> {
        ShapeError::Unfixed {
            variables: collect(self.lay, scheme.group(self.types).names.iter().copied()),
            wanted,
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
        let body = self.statement_at(level, last);
        let (types, scratch) = (self.types, self.scratch);
        // A body that never arrives returns nothing to check.
        if judge::is(self, types, body.upper, KType::NEVER) {
            return Ok(());
        }
        if let Some(refused) = judge::never_satisfies(self, types, scratch, body, ret) {
            return Err(ShapeError::ReturnNeverSatisfied {
                refused,
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
        let typed = self.viewed_tree(level, form, Written::Part(self.site(level, part)), typed)?;
        self.record_part(level, self.site(level, part), typed);
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
        self.viewed_tree(
            level,
            form,
            Written::Node(self.node_site(level, node)),
            typed,
        )
    }

    /// `typed`, the static type of a call, ascription, `EVAL` or member read at `site` of the shape
    /// at `level` the evaluator reads as `form`, where its tree is a view or a header: exactly the
    /// view's signature over a closed tree and the header's carried type over a closed header, and
    /// at most the view's application otherwise. Any other call, ascription or `EVAL` the run
    /// retypes by its fit that a `LET` binds whole is pinned at the paths through that binder
    /// ([`bound_at_its_paths`](Self::bound_at_its_paths)). Any other part's, or one that can never
    /// be a module's, as it is.
    fn viewed_tree(
        &mut self,
        level: usize,
        form: Form<'graph>,
        written: Written,
        typed: Interval,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        // An `EVAL`'s value is retyped by its fit, as a call's is, and is never a view.
        if matches!(form, Form::Eval(_)) {
            let named = self.binding_whole(level, written);
            return self.bound_at_its_paths(level, named, typed);
        }
        if !matches!(
            form,
            Form::Call(..) | Form::Apply(_) | Form::Ascribe(..) | Form::Member { .. }
        ) || judge::kind_above(self, types, scratch, typed.upper) != KKind::Signature
        {
            return Ok(typed);
        }
        // The binder whose whole right-hand side the part is keys a leaf its tree falls back to.
        let named = self.binding_whole(level, written);
        let tree = self.tree_form(trees::Scope::Chain(level), form, written, named)?;
        // A closed header is exactly the type the load computed for it with the run's own fit.
        if let Tree::Retyped { carried, .. } = tree
            && tree.closed()
        {
            return Ok(Interval::point(carried));
        }
        let Tree::View { application, .. } = tree else {
            // A member read is pinned where its rule reads it ([`read_through_paths`]).
            if matches!(form, Form::Member { .. }) {
                return Ok(typed);
            }
            return self.bound_at_its_paths(level, named, typed);
        };
        // A view at an application written over names is pinned at its paths, as its binding is.
        let Some(application) = judge::static_of(types, application) else {
            return self.bound_at_its_paths(level, named, typed);
        };
        if let Some(built) = (self.view_of(tree, written.site())?).and_then(|viewed| viewed.built) {
            return Ok(Interval::point(built.view.into()));
        }
        // A view a binder holds, or a `:|` writes in place, is the view rule over its shown
        // application, at every binding.
        let root = match named {
            Some(slot) => Some(Root::Binder(Binder {
                body: self.chain[level].shape.segment(),
                slot,
            })),
            None if matches!(form, Form::Ascribe(..)) => {
                let site = written.site();
                let statement = self.statement_holding(level, site) as u32;
                Some(Root::Inline { site, statement })
            }
            None => None,
        };
        if let Some(root) = root
            && let Some(shown) = self.shown_application(level, root, tree, application)?
        {
            let operands = [
                DeclaredType::Type(shown),
                DeclaredType::Type(application.into()),
            ];
            if let Some(view) = judge::ruled_one(self, types, scratch, Rule::View, &operands) {
                return Ok(Interval::point(view));
            }
        }
        // A `:|` the rule does not type is read off its operand's tree, which a binding of a
        // binder the tree reads re-runs.
        if matches!(form, Form::Ascribe(..)) {
            self.note_content_of(tree);
        }
        // Narrowed to the application through the meet rule, which a binding meets.
        let narrows =
            judge::is(self, types, typed.lower, KType::NEVER) && !judge::exact(self, types, typed);
        Ok(
            match narrows.then(|| judge::met(self, types, scratch, application.into(), typed.upper))
            {
                Some(Some(met)) => under(met),
                _ => typed,
            },
        )
    }

    /// The application a view at `application` of the tree `tree`, held by `root` in the shape at
    /// `level` — a binder, or the `:|` writing it in place — shows: `application` where it pins
    /// every head parameter, else pinned at the path through the root for each it leaves
    /// unpinned, a class over the view's own tree. `None` where a path is out of reach.
    fn shown_application(
        &mut self,
        level: usize,
        root: Root,
        tree: Tree<'o>,
        application: KType,
    ) -> Result<Option<Parametric>, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        if crate::scope::shown_view(types, scratch, application).is_some() {
            return Ok(Some(application.into()));
        }
        let view = matches!(root, Root::Inline { .. }).then_some(tree);
        let pinned = self.pinned(level, root, &[], application.into(), view)?;
        Ok(pinned.map(|(signature, pins)| types.signature_apply(scratch, signature, &pins)))
    }

    /// The binder whose whole right-hand side the part `written` of the shape at `level` is.
    fn binding_whole(&self, level: usize, written: Written) -> Option<Slot> {
        let shape = self.chain[level].shape;
        match written {
            Written::Part(site) => (0..shape.slots() as u32)
                .map(Slot)
                .find(|slot| (shape.rhs(*slot)).is_some_and(|part| self.site(level, part) == site)),
            Written::Node(_) => None,
        }
    }

    /// `typed`, the static type of the value the run retypes by its fit that a `LET` binds whole
    /// to `named` in the shape at `level`, pinned at the paths through that binder
    /// ([`at_its_paths`](Self::at_its_paths)). A value written in place has no binder for the run
    /// to read its paths through, so it stays as it is.
    fn bound_at_its_paths(
        &mut self,
        level: usize,
        named: Option<Slot>,
        typed: Interval,
    ) -> Result<Interval, ShapeError<'o>> {
        let Some(slot) = named else {
            return Ok(typed);
        };
        let binder = Binder {
            body: self.chain[level].shape.segment(),
            slot,
        };
        self.at_its_paths(level, Root::Binder(binder), &[], typed.upper, typed)
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
            Form::Block(nested) => {
                (self.block(level, nested, Entering::Plain)?).unwrap_or_else(unknown)
            }
            Form::Lambda(node) => {
                match shape.site_of_body(node).and_then(|site| shape.nested(site)) {
                    Some(body) => match self.callable(body) {
                        DeclaredType::Type(typed) => typed,
                        DeclaredType::Scheme(scheme) => {
                            let (typed, solution) =
                                self.instantiate(scheme, wanted, node.source)?;
                            if !self.unfilled {
                                let solution =
                                    self.instance(level, solution, typed.upper, node.source);
                                self.fix_born_instance(body, solution);
                            }
                            typed
                        }
                    },
                    None => unknown(),
                }
            }
            Form::Declaration => Interval::point(KType::NULL.into()),
            Form::Ascribe(node, mode) => self.ascribe(level, node, mode)?,
            Form::Eval(node) => self.eval(level, node)?,
            Form::Using(node) => self.using(level, node)?,
            Form::Match { node, under } => self.matched(level, node, under)?,
            Form::Try(node) => self.tried(level, node)?,
            Form::Catch(node) => self.caught(level, node)?,
            Form::Member {
                node,
                list,
                operand,
                label,
            } => {
                let peeked = self.peek(level, operand);
                let scheme = self.peeked_member_scheme(level, operand, peeked, label)?;
                match scheme {
                    Some(scheme) => self.member_instance(level, node, label, scheme, wanted)?,
                    None => {
                        let typed = self.narrow(level, node, list)?;
                        // An operand `peeked` cannot read is typed by the read's use.
                        let site = self.site(level, operand);
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
                .mention(self.site(level, name))
                .map(|mention| self.read_declared(level, mention.coordinate)),
            Form::Lambda(node) => (shape.site_of_body(node))
                .and_then(|site| shape.nested(site))
                .map(|body| self.callable(body)),
            // A member read at a call's head is read as it is, a quantified member by its scheme:
            // its site is recorded as read as it is, which the run reads as the head's mark.
            Form::Member {
                node,
                list,
                operand,
                label,
            } => {
                if !self.unfilled {
                    self.record_instance_read(
                        level,
                        self.node_site(level, node),
                        InstanceRead::AsIs,
                    );
                }
                let peeked = self.peek(level, operand);
                match self.peeked_member_scheme(level, operand, peeked, label)? {
                    Some((scheme, None)) => Some(DeclaredType::Scheme(scheme)),
                    // A scheme over a parameter the load cannot name: the run solves the call.
                    Some((_, Some(_))) => {
                        let typed = self.narrow(level, node, list)?;
                        self.record_part(level, self.site(level, head), typed);
                        return Ok(DeclaredType::Type(unknown()));
                    }
                    // An operand read only by typing it: the head is read as it is, as `part`
                    // reads a member short of its instance.
                    None if peeked.is_none() => {
                        let typed = self.narrow(level, node, list)?;
                        let typed = self.read_through_paths(level, operand, label, typed)?;
                        let form = of_part(self.chain[level].shape, head);
                        let typed = self.viewed_tree(
                            level,
                            form,
                            Written::Part(self.site(level, head)),
                            typed,
                        )?;
                        self.record_part(level, self.site(level, head), typed);
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
            self.record_minting(level, self.node_site(level, node));
            let tree = self.tree(level, &operand_part.value)?;
            let tree = resident(self.lay, tree);
            let site = self.node_site(level, node);
            // A view is typed by the view rule over the paths through its tree, which a binding of
            // a binder the tree reads closes ([`viewed_tree`](Self::viewed_tree)); one over a tree
            // no binding substitutes is read off its operand's tree, which a binding re-runs.
            match self.substitutes(*tree) {
                true => self.note_named_of(*tree),
                false => self.note_content_of(*tree),
            }
            if tree.closed() {
                self.chain[level].closed += 1;
                if let Some(unit) = self.fixing() {
                    unit.closed += 1;
                }
            }
            self.record_view(level, site, tree);
            if let Some(declared) = declared {
                // What the operand carries, read off its tree, beside the view's tree: an open
                // view over an application written over names, which a binding substitutes.
                let (viewed, seen, revealed) = self.view_tree_held(declared, *tree, site)?;
                let signature = judge::static_of(types, declared);
                if let (Some(member), Some(signature)) = (revealed, signature) {
                    return Err(ShapeError::ViewReveals {
                        member: member.symbol(),
                        ascribed: signature,
                        at: node.source,
                    });
                }
                self.record_view_tree(level, site, resident(self.lay, viewed));
                // Over a closed tree the load lays every type the view door computes from types
                // alone, so the run's view interns none of its own. The view's members are its
                // underlying's.
                if let Some(signature) = signature
                    && let Some(bare) = tree.bare().digest()
                {
                    // A closed tree's operand is closed where it is exact, so no binding closes
                    // it: reading it closed judges nothing.
                    let exact = judge::exact(self, types, typed)
                        .then(|| judge::static_of(types, typed.upper))
                        .flatten();
                    debug_assert!(
                        !judge::exact(&NoMarks, types, typed) || exact.is_some(),
                        "an exact operand of a closed tree is closed"
                    );
                    debug_assert!(
                        exact.is_none() || seen.is_none() || exact == seen,
                        "a tree reads the type its operand carries"
                    );
                    let underlying = match viewed {
                        Tree::View { underlying, .. } => *underlying,
                        _ => *tree,
                    };
                    // The view's tree fit the source at what its tree shows; else by its exact
                    // type, a module known to the tree by no signature its own.
                    let (carried, types_of) = match (seen, exact, tree.bare()) {
                        (None, Some(exact), Tree::Body { .. }) => {
                            #[cfg(test)]
                            count(|counts| counts.fits += 1);
                            let operand = Operand {
                                held: Some(exact),
                                digest: Some(bare),
                                bare: Some(Bare {
                                    signature: Some(exact),
                                    sealing: None,
                                }),
                            };
                            let fit =
                                view::exposure(operand, signature, signature, &[], types, scratch);
                            (views::Carried::Typed(exact), fit.ok())
                        }
                        _ => (
                            views::Carried::Tree(underlying),
                            self.view_of(viewed, site)?,
                        ),
                    };
                    if let Some(types_of) = types_of {
                        self.lay(&types_of, carried, site)?;
                        if let Tree::Body { body, .. } = *tree
                            && tree.closed()
                            && let Some(picks) = self.picks(body, &types_of)
                        {
                            self.record_picks(level, site, picks);
                        }
                    }
                }
            }
            return self.viewed(level, node, typed, declared);
        }
        // A hold is see-through: a module a name chain reaches, held in place, is pinned at the
        // paths through that chain. One a `LET` binds whole is pinned at its binder's.
        let paths = match self.binds_whole(level, node) {
            true => None,
            false => self.held_chain(level, &operand_part.value),
        };
        let at = match &paths {
            Some((binder, chain)) => PinnedAt::Operand(*binder, chain),
            None => PinnedAt::Nowhere,
        };
        let site = self.node_site(level, node);
        let source = node.source;
        self.held(level, typed, &ascribed.value, site, at, move |refused| {
            ShapeError::AscriptionNeverSatisfied {
                refused,
                at: source,
            }
        })
    }

    /// Whether a `LET` of the shape at `level` binds the ascription `node` whole.
    fn binds_whole(&self, level: usize, node: &'graph KExpression<'graph>) -> bool {
        let shape = self.chain[level].shape;
        (0..shape.slots() as u32).map(Slot).any(|slot| {
            (shape.rhs(slot)).is_some_and(|part| match of_part(shape, part) {
                Form::Ascribe(bound, _) => std::ptr::eq(bound, node),
                _ => false,
            })
        })
    }

    /// What a view of `viewed`'s signature over the module the body `body` births carries for each
    /// keyworded member, by its registrations' positions in its run: `None` where a registration's
    /// shape is not closed, or the view's fit leaves the source's side unknown.
    fn picks(&mut self, body: Segment, viewed: &Exposure<'_, '_>) -> Option<&'o [Pick<'o>]> {
        let registered = self.registered_run(body)?;
        let picks = view::picks(viewed, &registered, self.types, self.scratch)?;
        Some(self.laid_picks(&picks))
    }

    /// The shapes the registrations of the module the body `body` births register at, in their
    /// run's order: `None` where one is not closed.
    fn registered_run(&mut self, body: Segment) -> Option<BumpVec<'p, DeclaredType<KType>>> {
        let scratch = self.scratch;
        let shape = self.shapes.shape_at(body);
        // A module's registrations follow its named members in slot order.
        let mut slots = BumpVec::new_in(scratch);
        slots.extend(
            shape
                .registrations()
                .iter()
                .map(|registration| registration.slot),
        );
        slots.sort_unstable();
        let mut registered = BumpVec::with_capacity_in(slots.len(), scratch);
        for slot in slots.iter() {
            match self.registered_cell(shape, *slot) {
                Static::Closed(registration) => registered.push(registration.shape),
                Static::Rigid { .. } | Static::Unknown => return None,
            }
        }
        Some(registered)
    }

    fn laid_picks(&self, picks: &[(u16, BumpVec<'_, u32>)]) -> &'o [Pick<'o>] {
        let lay = self.lay;
        lay.fill(picks.len(), |at| Pick {
            member: picks[at].0,
            offered: collect(lay, picks[at].1.iter().copied()),
        })
    }

    /// Whether a value of one of the types `uppers` never arrives, so the rule over them refuses
    /// nothing and is `Never`: a discrete outcome, the witness's answer over names.
    fn never_arrives(&self, uppers: &[Parametric]) -> bool {
        (uppers.iter()).any(|upper| judge::is(self, self.types, *upper, KType::NEVER))
    }

    /// `result`, the type of a rule over values of the types `uppers` where each arrives, `Never`
    /// where one never does, at load and under every binding alike ([`judge::arrives`]).
    fn arriving(&self, uppers: &[Parametric], result: Interval) -> Interval {
        judge::arrives(self, self.types, self.scratch, uppers, result)
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
        if self.never_arrives(&[typed.upper]) {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let viewed = self.viewed_arriving(level, node, typed, declared)?;
        Ok(self.arriving(&[typed.upper], viewed))
    }

    /// [`viewed`](Self::viewed) where the operand arrives.
    fn viewed_arriving(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        typed: Interval,
        declared: Option<Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        if let Some(refused) =
            judge::never_satisfies(self, types, scratch, typed, KType::EMPTY_SIGNATURE.into())
        {
            return Err(ShapeError::NotAModule {
                refused,
                at: node.source,
            });
        }
        let Some(declared) = declared else {
            return Ok(unknown());
        };
        if judge::module_decided(self, types, scratch, declared) == Some(false) {
            return Err(ShapeError::NotASignature {
                ascribed: declared,
                at: node.source,
            });
        }
        if judge::kind_above(self, types, scratch, typed.upper) != KKind::Signature
            || judge::kind_above(self, types, scratch, declared) != KKind::Signature
        {
            return Ok(under(declared));
        }
        if judge::fits(self, types, scratch, typed.upper, declared) {
            self.record_settled(level, self.node_site(level, node));
        }
        // An exact operand that does not fit its view is refused; one over names is decided by
        // the binding that closes it.
        if judge::exact(self, types, typed)
            && judge::fit_decided(self, types, scratch, typed.upper, declared) == Some(false)
        {
            return Err(ShapeError::AscriptionNeverSatisfied {
                refused: NeverSatisfied {
                    value: Interval::point(typed.upper),
                    wanted: declared,
                },
                at: node.source,
            });
        }
        Ok(under(declared))
    }

    /// `LET <name> <type> = <value>`, the binder at `member` of the shape at `level`, whose value
    /// is `typed`: held to `annotation` as `:!` holds its operand, settled by the annotation's site,
    /// and pinned at the paths through the binder ([`at_its_paths`](Self::at_its_paths)).
    fn annotated(
        &mut self,
        level: usize,
        member: Slot,
        annotation: &'graph ExpressionPart<'graph>,
        typed: Interval,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let binder = Binder {
            body: shape.segment(),
            slot: member,
        };
        let site = self.site(level, annotation);
        let at = PinnedAt::Binder(binder);
        self.held(level, typed, annotation, site, at, move |refused| {
            let statement = shape.position_of(member).statement_index();
            ShapeError::AnnotationNeverSatisfied {
                refused,
                at: shape.body()[statement.expect("an annotated binder is a statement's")].source,
            }
        })
    }

    /// A value of the static type `typed` held to the type part `part`: that type, exactly where
    /// the retype makes it so ([`retyped_to`]), pinned at the paths `at` names
    /// ([`at_its_paths`](Self::at_its_paths)), and settled at `site` where `typed`'s upper end
    /// lies under it. What the retype re-stamps at is recorded at `site`. A module exact at a
    /// closed type is exactly what the run retypes it to, pinned by the run's own
    /// [`pinned_at_fit`]. A refusal is worded by `refused`.
    fn held(
        &mut self,
        level: usize,
        typed: Interval,
        part: &'graph ExpressionPart<'graph>,
        site: Site,
        at: PinnedAt<'_>,
        refused: impl FnOnce(NeverSatisfied) -> ShapeError<'o>,
    ) -> Result<Interval, ShapeError<'o>> {
        // A value that never arrives has nothing to check.
        if self.never_arrives(&[typed.upper]) {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let (held, restamp) = self
            .held_arriving(level, typed, part, site)
            .map_err(refused)?;
        let (paths, in_place) = match at {
            PinnedAt::Nowhere => (None, false),
            PinnedAt::Binder(binder) => (Some((binder, &[][..])), false),
            PinnedAt::Operand(binder, chain) => (Some((binder, chain)), true),
        };
        let pinned = match (paths, self.declared(level, part)) {
            (Some((binder, chain)), Some(declared)) => {
                self.at_its_paths(level, Root::Binder(binder), chain, declared, held)?
            }
            _ => held,
        };
        // A hold pinned in place is re-stamped at its pinned point, as an exact closed module is.
        let restamp = match in_place && pinned != held {
            true => Some(Restamp {
                how: Restamping::Point,
                at: pinned.upper,
            }),
            false => restamp,
        };
        if let Some(restamp) = restamp {
            self.record_retype(level, site, restamp);
        }
        Ok(self.arriving(&[typed.upper], pinned))
    }

    /// [`held`](Self::held) where the value arrives, beside what the retype re-stamps at.
    fn held_arriving(
        &mut self,
        level: usize,
        typed: Interval,
        part: &'graph ExpressionPart<'graph>,
        site: Site,
    ) -> Result<(Interval, Option<Restamp>), NeverSatisfied> {
        let Some(declared) = self.declared(level, part) else {
            return Ok((unknown(), None));
        };
        let (types, scratch) = (self.types, self.scratch);
        if let Some(refused) = judge::never_satisfies(self, types, scratch, typed, declared) {
            return Err(refused);
        }
        if judge::fits(self, types, scratch, typed.upper, declared) {
            self.record_settled(level, site);
        }
        // Only a module may be exact at a signature, so a value no binding makes one asks nothing.
        let module = judge::kind_above(self, types, scratch, typed.upper) == KKind::Signature;
        // A module over names is pinned at its paths instead ([`held`](Self::held)).
        let exact = ((module && judge::exact(self, types, typed))
            .then(|| judge::static_of(types, typed.upper)))
        .flatten()
        .filter(|exact| exact.kind_of(types) == KKind::Signature);
        if let Some(exact) = exact
            && let Some(closed) = judge::static_of(types, declared)
        {
            // Signatures meet to a set, never to `Never`, so only an exact module can be refused.
            let signature = matches!(
                judge::closed_type_node(types, closed),
                TypeNode::Signature { .. }
                    | TypeNode::SignatureApply { .. }
                    | TypeNode::SignatureMeet { .. }
            );
            if signature && pinned_at_fit(types, scratch, exact, closed).is_none() {
                return Err(NeverSatisfied {
                    value: Interval::point(exact.into()),
                    wanted: closed.into(),
                });
            }
            let carried = retype_target(types, scratch, exact.into(), closed)
                .as_type()
                .expect("a module's type is a type");
            self.lay_carried(carried);
            let at = carried.into();
            let how = Restamping::Point;
            return Ok((Interval::point(carried.into()), Some(Restamp { how, at })));
        }
        let restamp = judge::restamp(self, types, scratch, declared);
        Ok((judge::retyped(self, types, scratch, declared), restamp))
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
        let Some(body) = shape.site_of_body(node).and_then(|site| shape.nested(site)) else {
            return Ok(unknown());
        };
        // A surfaced key through a header over a module the load sees all of offers the overloads
        // the load picks here, by position.
        if !body.registrations().is_empty()
            && let Tree::Retyped {
                carried, source, ..
            } = self.tree(level, &operand.value)?
            && let Tree::Body { body: module, .. } = *source
            && source.closed()
            && let Some(registered) = self.registered_run(module)
            && let Some(picks) = layout::header_picks(
                carried,
                &registered,
                (self.types, self.scratch),
                &mut |a, b| judge::same(self, self.types, a, b),
                &mut |registered, declared| {
                    judge::fit_decided(self, self.types, self.scratch, registered, declared)
                },
            )
        {
            let picks = self.laid_picks(&picks);
            self.record_picks(level, self.node_site(level, node), picks);
        }
        let tag = self.innermost();
        Ok(self
            .block(level, body, Entering::Surfaced(typed, tag))?
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
        if self.never_arrives(&[typed.upper]) {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let evaluated = self.evaluated(level, node, typed, &operand.value, &declared.value)?;
        Ok(self.arriving(&[typed.upper], evaluated))
    }

    /// [`eval`](Self::eval) where its operand, of the static type `typed`, arrives.
    fn evaluated(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        typed: Interval,
        operand: &'graph ExpressionPart<'graph>,
        declared: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        // Every `EVAL` enters a frame whose code the load may not see.
        self.exit(level);
        let (types, scratch) = (self.types, self.scratch);
        if let Some(refused) =
            judge::never_satisfies(self, types, scratch, typed, KType::ANY_CODE.into())
        {
            return Err(ShapeError::NotCode {
                refused,
                at: node.source,
            });
        }
        let Some(declared) = self.declared(level, declared) else {
            return Ok(unknown());
        };
        // Code that never arrives returns nothing to check.
        if let Some(code) = self.traced(level, operand)
            && code != KType::NEVER.into()
            && let Some(refused) = judge::never_satisfies(
                self,
                types,
                scratch,
                Interval {
                    lower: KType::NEVER.into(),
                    upper: code,
                },
                declared,
            )
        {
            return Err(ShapeError::EvalNeverSatisfied {
                refused,
                at: node.source,
            });
        }
        Ok(judge::retyped(self, types, scratch, declared))
    }

    /// The type the type part `part` of the shape at `level` denotes, where the load knows it.
    fn declared(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Parametric> {
        let typed = self.type_cell(self.chain[level].shape, self.site(level, part));
        match self.resolved_type(typed) {
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
            // Code composed where it is born may be any code of its kind.
            ExpressionPart::QuotedExpression(node) => match shape.nested(self.site(level, part)) {
                None => unknown(),
                Some(code) if code.composing() => {
                    self.spliced(level, node, code)?;
                    under(code.code_type().into())
                }
                Some(code) => Interval::point(code.code_type().into()),
            },
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                match self.resolved_type(self.type_cell(shape, shape.site(part))) {
                    Static::Closed(handle) => {
                        Interval::point(KType::of_kind(handle.kind_of(types)).into())
                    }
                    // Over names, the kind each binding gives it, one type under every binding.
                    Static::Rigid { value, .. } => {
                        Interval::point(judge::kind_value(self, types, scratch, value))
                    }
                    Static::Unknown => under(KType::ANY_TYPE.into()),
                }
            }
            ExpressionPart::ListLiteral(items) => {
                let element = wanted
                    .and_then(|wanted| judge::piece(self, types, scratch, wanted, Step::Element));
                // An item stands for itself, and a spread for its list's elements.
                let mut typed = BumpVec::with_capacity_in(items.len(), scratch);
                let mut elements = BumpVec::with_capacity_in(items.len(), scratch);
                for item in items.iter() {
                    match item {
                        ListItem::Part(item) => {
                            let each = self.part_at(level, item, element)?;
                            typed.push(each);
                            elements.push(each);
                        }
                        ListItem::Spread(source) => {
                            let list = self.part(level, source)?;
                            typed.push(list);
                            elements.push(self.spread_elements(level, source, list)?);
                        }
                    }
                }
                self.record_composite(
                    level,
                    self.site(level, part),
                    items.iter().map(ListItem::part).zip(typed.iter().copied()),
                );
                list_interval(self, types, scratch, &elements)
            }
            // A rendered brace's value is written into the text, so its type constrains nothing.
            ExpressionPart::Template(template) => {
                let mut typed = BumpVec::with_capacity_in(template.slots.len(), scratch);
                for slot in template.slots.iter() {
                    typed.push(self.part(level, slot.part())?);
                }
                let slots = template.slots.iter().map(TemplateSlot::part);
                self.record_composite(
                    level,
                    self.site(level, part),
                    slots.zip(typed.iter().copied()),
                );
                let mut kept = BumpVec::with_capacity_in(typed.len(), scratch);
                let each = template.slots.iter().zip(typed.iter());
                kept.extend(
                    each.filter(|(slot, _)| slot.is_kept())
                        .map(|(_, typed)| *typed),
                );
                if kept.is_empty() {
                    return Ok(Interval::point(KType::STR.into()));
                }
                let values = list_interval(self, types, scratch, &kept);
                let strings: Parametric =
                    list_type(types, scratch, [KType::STR].into_iter()).into();
                let [names, values_name] = template_fields();
                let record = |values| [(names, strings), (values_name, values)].into_iter();
                Interval {
                    lower: record_type(types, scratch, record(values.lower)),
                    upper: record_type(types, scratch, record(values.upper)),
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                let values = wanted
                    .and_then(|wanted| judge::piece(self, types, scratch, wanted, Step::Value));
                let mut typed = BumpVec::with_capacity_in(pairs.len(), scratch);
                for (key, value) in pairs.iter() {
                    let key = self.part(level, key)?;
                    typed.push((key, self.part_at(level, value, values)?));
                }
                let each = pairs.iter().zip(typed.iter());
                self.record_composite(
                    level,
                    self.site(level, part),
                    each.flat_map(|((key, value), (k, v))| [(key, *k), (value, *v)]),
                );
                let (mut keys, mut values) = (
                    BumpVec::with_capacity_in(typed.len(), scratch),
                    BumpVec::with_capacity_in(typed.len(), scratch),
                );
                for (key, value) in typed.iter() {
                    keys.push(*key);
                    values.push(*value);
                }
                // A key is exact where its ends meet, so the union of the lower ends is the keys'
                // fill wherever the run fills them: only the values read through the fill.
                let key = |end: fn(&Interval) -> Parametric| {
                    let mut ends = BumpVec::with_capacity_in(keys.len(), scratch);
                    ends.extend(keys.iter().map(end));
                    types.union_of(scratch, &ends)
                };
                let (lower, upper) = filled_ends(self, types, scratch, &values, &keys);
                Interval {
                    lower: types.dict(key(|k| k.lower), lower),
                    upper: types.dict(key(|k| k.upper), upper),
                }
            }
            ExpressionPart::RecordLiteral(fields) => {
                let mut typed = BumpVec::with_capacity_in(fields.len(), scratch);
                for (name, value) in fields.iter() {
                    let field = wanted.and_then(|wanted| {
                        judge::piece(self, types, scratch, wanted, Step::Field(*name))
                    });
                    typed.push((*name, self.part_at(level, value, field)?));
                }
                let each = fields.iter().zip(typed.iter());
                self.record_composite(
                    level,
                    self.site(level, part),
                    each.map(|((_, value), (_, typed))| (value, *typed)),
                );
                Interval {
                    lower: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.lower))),
                    upper: record_type(types, scratch, typed.iter().map(|(n, v)| (*n, v.upper))),
                }
            }
            ExpressionPart::Keyword(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::MarkedUse(..)
            | ExpressionPart::FilledUse(..) => unknown(),
            _ => unreachable!("a name is a `Form::Name`, typed by `name`"),
        })
    }

    /// Refuse a splice of the composing quote `node`, of code shape `code`, whose operand's static
    /// type can never be a list of code: each operand is the quote's `$` capture of its name, read
    /// where the quote is written.
    fn spliced(
        &self,
        level: usize,
        node: &'graph KExpression<'graph>,
        code: &BodyShape<'_>,
    ) -> Result<(), ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let codes = types.list(KType::ANY_CODE);
        let mut refused = Ok(());
        each_splice(node, &mut |name, at| {
            let found = code.captures().iter().find(|capture| {
                capture.name == BinderSymbol::Value(name)
                    && capture.mark == Some(crate::parse::Mark::DOLLAR)
            });
            let Some(CaptureSource::Read(coordinate)) = found.map(|capture| capture.source) else {
                return;
            };
            // A quantified function, read anywhere but a call's head, the builder refused.
            let DeclaredType::Type(typed) = self.read_declared(level, coordinate) else {
                return;
            };
            if refused.is_ok() && judge::lower_end_outside(self, types, scratch, typed, codes) {
                refused = Err(ShapeError::SpliceNeverCode {
                    value: typed.upper,
                    at,
                });
            }
        });
        refused
    }

    /// The static type of the elements a spread of `source`, typed `list`, lays into a list
    /// literal: each end's element where that end is a list, `Never` below and `Any` above where it
    /// is none. Refused where the lower end lies outside every list.
    fn spread_elements(
        &self,
        level: usize,
        source: &ExpressionPart<'_>,
        list: Interval,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        if judge::lower_end_outside(self, types, scratch, list, KType::LIST_OF_ANY) {
            return Err(ShapeError::SpreadNeverAList {
                value: list.upper,
                at: self.source(level, self.site(level, source)),
            });
        }
        let element = |end: Parametric, missing: KType| {
            judge::piece(self, types, scratch, end, Step::Element).unwrap_or(missing.into())
        };
        Ok(Interval {
            lower: element(list.lower, KType::NEVER),
            upper: element(list.upper, KType::ANY),
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
        let site = self.site(level, part);
        let Some(mention) = self.chain[level].shape.mention(site) else {
            return Ok(unknown());
        };
        Ok(match self.read_declared(level, mention.coordinate) {
            DeclaredType::Type(typed) => typed,
            DeclaredType::Scheme(scheme) => {
                let at = self.source(level, site);
                let (typed, solution) = self.instantiate(scheme, wanted, at)?;
                if !self.unfilled {
                    let solution = self.instance(level, solution, typed.upper, at);
                    self.record_instance_read(level, site, InstanceRead::Solved(solution));
                }
                typed
            }
        })
    }

    /// The static type of what `coordinate`, read in the shape at `level`, holds, a quantified
    /// callable's its scheme. A capture along the chain reads its source's, each path variable
    /// there no path of whose class this shape reaches read through its bounds. One crossing into a
    /// quote's code is typed over the code's capture ([`crossing`](Self::crossing)), else read
    /// through its bounds.
    fn read_declared(&self, level: usize, coordinate: Coordinate) -> Bound {
        let Followed {
            landing,
            crossed,
            entered,
        } = self.follow(level, coordinate);
        if crossed
            && let Some(entered) = entered
            && let Some(typed) = self.crossing(entered, landing)
        {
            return DeclaredType::Type(typed);
        }
        self.read_landed(level, landing)
    }

    /// The type of a read crossing into a quote's code through its capture `entered`, landing at
    /// `landing`, written over the capture, where the code binds the capture itself: a module
    /// binder's capture holds the module whose carried type a binding of the capture names
    /// ([`DerivedTerm::Carried`]). Where the binding binds the capture, or its source, the read is
    /// what it gives that type, and a source exact at a closed type is the type every module it
    /// holds carries. `None` for any other capture, read as its source is.
    ///
    /// [`DerivedTerm::Carried`]: crate::scope::DerivedTerm::Carried
    fn crossing(&self, entered: Unknown, landing: Landing) -> Option<Interval> {
        let Address::Capture(capture) = entered.at else {
            return None;
        };
        let bound = self.bound_through(entered);
        match self.shape_at(entered.segment).captures()[capture.index()].name {
            BinderSymbol::Value(name) => {
                let Landing::Slot { level, slot } = landing else {
                    return None;
                };
                let DeclaredType::Type(source) = self.binder_at(level, slot) else {
                    return None;
                };
                self.module_unknown(level, slot, DeclaredType::Type(source))?;
                let exact = (source.lower == source.upper)
                    .then(|| judge::static_of(self.types, source.upper))
                    .flatten();
                Some(Interval::point(match (bound, exact) {
                    (Some(crate::scope::Bound::Module { carried, .. }), _) => {
                        self.note_named_unknown(entered);
                        carried.into()
                    }
                    (_, Some(exact)) => exact.into(),
                    _ => {
                        self.note_named_unknown(entered);
                        self.derive(crate::scope::DerivedTerm::Carried {
                            binder: entered,
                            name,
                        })?
                    }
                }))
            }
            _ => None,
        }
    }

    /// The static type of what a coordinate read in the shape at `level` lands at.
    fn read_landed(&self, level: usize, landing: Landing) -> Bound {
        match landing {
            Landing::Builtin(index) => self
                .builtins
                .get(index)
                .ktype()
                .map(|ktype| Interval::point(ktype.into())),
            Landing::Slot { level: at, slot } if at < level => {
                self.sourced_bound(level, self.binder_at(at, slot))
            }
            Landing::Slot { level, slot } => self.binder_at(level, slot),
            Landing::Open { .. } => DeclaredType::Type(unknown()),
        }
    }

    /// Follow `coordinate`, read in the shape at `level`, to where it lands on the chain, by the
    /// shapes' one rule: whether it crossed into a quote's code is whether it took a capture of a
    /// shape rooting a chain through to a shape outside it.
    fn follow(&self, level: usize, coordinate: Coordinate) -> Followed {
        let mut entered = None;
        let landed = follow(self, level, coordinate, |at, capture| {
            if self.chain[at].shape.kind() == ShapeKind::Code && entered.is_none() {
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
            Some(Lands::Offered { at, .. } | Lands::Entry { at, .. }) => {
                (Landing::Open { hole: false }, Some(at))
            }
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

    /// `(head argument)`: a construction's identity when the head is a type the load knows, at
    /// most the union a tag projection head reaches its variant through; a read's `Result` when
    /// it is a list, a dict or a string ([lookups.rs](walk/lookups.rs)); a call by name's
    /// [judged](Self::called) return when the head is a function; else `[Never, Any]`.
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
            let owner = self.reached_owner(level, head);
            let payload = self.part(level, argument)?;
            // An unsolved family lies under the bare family or union; a misfit faults, so its
            // upper end is the bare head.
            let upper =
                judge::construction_upper(self, types, scratch, (identity, payload.upper), owner);
            let never = Parametric::from(KType::NEVER);
            let lower = judge::construction(self, types, scratch, (identity, payload.lower))
                .unwrap_or(never);
            self.record_composite(
                level,
                self.node_site(level, node),
                [(argument, payload)].into_iter(),
            );
            return Ok(Interval { lower, upper });
        }
        if let Some(Static::Rigid {
            value: head_type, ..
        }) = self.head_typed(level, head)
        {
            return self.built_over_names(level, node, (head, head_type), argument);
        }
        if let DeclaredType::Type(typed) = callee
            && self.applies_over_names(typed)
        {
            return self.applied_over_names(level, node, typed, argument);
        }
        // A list, a dict or a string at the head is read, never called: no route.
        if let Some(read) = self.read(level, node, (head, callee), argument)? {
            return Ok(read);
        }
        self.route_by_name(level, node, callee, None);
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
            DeclaredType::Type(interval) => (
                DeclaredType::Type(interval.upper),
                judge::exact(self, types, interval),
            ),
            DeclaredType::Scheme(scheme) => (DeclaredType::Scheme(scheme), true),
        };
        let view = function.view(types);
        // A run-bound head's call is typed by its rule ([`applied_over_names`]); any other
        // variable's is the run's.
        let body = match function {
            DeclaredType::Type(upper) => judge::node_unless_variable(types, upper)?,
            DeclaredType::Scheme(_) => view.body,
        };
        let TypeNode::KFunction { params, ret } = body else {
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
            solving: self.solving(shape.handle),
        })
    }

    /// The static type `payload` of a call by name's argument laid onto `callee`'s slots: each
    /// slot the field of its name at each end, `Never` below and `Any` above where that end is no
    /// record. Records are width-superset, so every carried record lies above the lower end and
    /// names no field it lacks, and lies under the upper end and names every field it names:
    /// `None` where the lower end is a record lacking a parameter, or the upper end a record naming
    /// a field no parameter declares — the call can never name the parameters exactly. An end
    /// that is a variable is laid through the rule a binding closes
    /// ([`laid_over_names`](Self::laid_over_names)).
    fn laid_out(&self, callee: Callee<'_>, payload: Interval) -> Option<&'p [Interval]> {
        let types = self.types;
        let variable = |end: Parametric| judge::node_unless_variable(types, end).is_none();
        if variable(payload.lower) || variable(payload.upper) {
            return self.laid_over_names(callee, payload);
        }
        let fields = |end: Parametric| match judge::node_unless_variable(types, end)? {
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

    /// [`laid_out`](Self::laid_out) where an end of `payload` is a variable, whose binding decides
    /// whether it is a record: each slot's ends the outputs of [`Rule::LaidOut`], whose closed rule
    /// is the closed layout, so they are the same at load and under every binding. `None` where
    /// every binding, or the witness, can never name the parameters.
    fn laid_over_names(&self, callee: Callee<'_>, payload: Interval) -> Option<&'p [Interval]> {
        let (types, scratch) = (self.types, self.scratch);
        let any = Parametric::from(KType::ANY);
        let mut fields = BumpVec::with_capacity_in(callee.names.len(), scratch);
        fields.extend(callee.names.iter().map(|name| (*name, any)));
        let names = types.record(scratch, &fields);
        let operands = [payload.lower, payload.upper, names].map(DeclaredType::Type);
        let rule = Rule::LaidOut {
            slots: callee.names.len() as u8,
        };
        if judge::defined(self, types, scratch, rule, &operands).0 == Some(false) {
            return None;
        }
        let mut undecided = BumpVec::with_capacity_in(callee.names.len() * 2, scratch);
        undecided
            .extend((0..callee.names.len()).flat_map(|_| [Parametric::from(KType::NEVER), any]));
        let ruled = judge::ruled(self, types, scratch, (rule, &operands), &undecided)?;
        // The outputs come in the record's field order: each name's pair read by its position.
        let Some(TypeNode::Record { fields: ordered }) = judge::node_unless_variable(types, names)
        else {
            unreachable!("a record type is a record")
        };
        let mut laid = BumpVec::with_capacity_in(callee.names.len(), self.scratch);
        for name in callee.names {
            let at = (ordered.iter())
                .position(|(field, _)| field.symbol() == name.symbol())
                .expect("each name is a field of the record over the names");
            laid.push(Interval {
                lower: ruled[2 * at],
                upper: ruled[2 * at + 1],
            });
        }
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
        let mut uppers = BumpVec::new_in(self.scratch);
        let called = self.called_arriving(level, callee, argument, node, &mut uppers)?;
        Ok(self.arriving(&uppers, called))
    }

    /// [`called`](Self::called) where its argument arrives, each upper end the call runs only
    /// where it arrives pushed to `uppers`.
    fn called_arriving(
        &mut self,
        level: usize,
        callee: Callee<'p>,
        argument: &'graph ExpressionPart<'graph>,
        node: &'graph KExpression<'graph>,
        uppers: &mut BumpVec<'_, Parametric>,
    ) -> Result<Interval, ShapeError<'o>> {
        let never = Interval::point(KType::NEVER.into());
        let at_most = |pass: &Self| Ok(under(pass.returned(callee.shape, None)));
        // The argument's whole static type, where it was typed whole.
        let mut whole = None;
        let (payload, arguments, contributions) = match (callee.function, argument) {
            (DeclaredType::Scheme(_), ExpressionPart::RecordLiteral(fields)) => {
                match self.literal(level, callee, argument, fields, node)? {
                    Some(laid) => {
                        uppers.extend(laid.1.iter().map(|each| each.upper));
                        laid
                    }
                    None => return Ok(never),
                }
            }
            _ => {
                let payload = self.part_at(level, argument, callee.wanted)?;
                uppers.push(payload.upper);
                if self.never_arrives(&[payload.upper]) {
                    return Ok(never);
                }
                // A callee known at most may bind a function whose parameters admit more.
                let inexact = !callee.exact
                    && porous_site(self.types, payload)
                    && !self.holds_nothing_minted_part(level, argument, payload)?;
                let Some(arguments) = self.laid_out(callee, payload) else {
                    return match callee.exact {
                        true => Err(self.never(callee, payload, node)),
                        false => {
                            if inexact {
                                self.exit(level);
                            }
                            at_most(self)
                        }
                    };
                };
                uppers.extend(arguments.iter().map(|each| each.upper));
                if self.never_arrives(&uppers[1..]) {
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
                (payload, arguments, contributions)
            }
        };
        if !self.unfilled && (contributions.iter()).any(|each| !matches!(each, Static::Unknown)) {
            let recorded = collect(self.lay, contributions.iter().copied());
            self.record_named(level, self.site(level, argument), recorded);
        }
        let (types, scratch) = (self.types, self.scratch);
        let minted_free = self.minted_free_fields(level, callee.names, argument, whole)?;
        if self.slot_exits(callee.shape, arguments, minted_free) {
            self.exit(level);
        }
        let read = self.solved_from(callee.solving, arguments, contributions);
        let judged = judge::verdict(self, types, scratch, callee.shape, read);
        if judged.verdict == Verdict::Never {
            return Err(self.never(callee, payload, node));
        }
        if !self.unfilled && self.store.traces() {
            self.trace_by_name(level, node, judged.solve.intervals());
        }
        Ok(self.registered_return(callee.shape, judged.solve.intervals(), false))
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
        let solving = callee.solving;
        let mut contributions = BumpVec::with_capacity_in(arguments.len(), self.scratch);
        for (slot, argument) in arguments.iter().enumerate() {
            contributions.push(match solving.get(slot) == Some(&true) && typed(slot) {
                true => (self.contribution(level, argument.upper, at)).unwrap_or(Static::Unknown),
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
            return Err(self.never(callee, under(uppers(&typed)), node));
        }
        let mut arriving = BumpVec::new_in(scratch);
        arriving.extend((typed.iter().flatten()).map(|typed| typed.upper));
        if self.never_arrives(&arriving) {
            let never = Interval::point(KType::NEVER.into());
            self.record_part(level, self.site(level, argument), never);
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
            let made = match self.instances(
                Some(callee.shape),
                callee.solving,
                &sites,
                &arguments,
                contributions,
            ) {
                Ok(made) => made,
                Err(None) => return Err(self.never(callee, under(uppers(&typed)), node)),
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
        let fields = collect(scratch, arguments.iter().map(|each| each.upper));
        let recorded = self.arriving(fields, payload);
        self.record_part(level, self.site(level, argument), recorded);
        Ok(Some((payload, arguments.leak(), contributions)))
    }

    /// `CallNeverSatisfied` for the call by name `node` of `callee`, read through its bounds, over
    /// an argument of the static type `payload` [shown against](self::shown_against) its
    /// parameters, which the refusal names beside it unless `callee` is quantified and names them
    /// itself.
    fn never(
        &self,
        callee: Callee<'_>,
        payload: Interval,
        node: &KExpression<'_>,
    ) -> ShapeError<'o> {
        let (types, scratch) = (self.types, self.scratch);
        let TypeNode::KFunction { params, .. } = callee.function.view(types).body else {
            unreachable!("a callee is a function")
        };
        let parameters = record_type(types, scratch, params.iter());
        ShapeError::CallNeverSatisfied {
            callee: callee.function,
            payload,
            parameters,
            at: node.source,
        }
    }

    /// `ret`, a return under its own group, read through that group's `intervals` — each variable
    /// at `[Never, bound]` where there are none — keeping every lexical variable.
    fn through(&self, ret: Parametric, intervals: Option<&[Interval]>) -> Parametric {
        // The group's own variables at their solve's intervals: a substitution, which commutes.
        judge::read_points(
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

    /// The owner a construction's head `head` reaches its variant through: where it is a tag
    /// projection whose owner the load knows closed and peels to a union ([`tag_union`]). `None`
    /// for every other head.
    fn reached_owner(&self, level: usize, head: &'graph ExpressionPart<'graph>) -> Option<KType> {
        let ExpressionPart::SigiledTypeExpr(node) = head else {
            return None;
        };
        let (owner, label) = tag_projection(node)?;
        // Read only where the head names a closed variant, whose owner is closed too.
        let owner = self
            .declared(level, owner)
            .and_then(|owner| judge::static_of(self.types, owner))?;
        let union = tag_union(self.types, self.scratch, owner)?;
        debug_assert_eq!(
            self.types.union_member_named(union, label.symbol()),
            self.head_handle(level, head),
            "the variant a tag projection names is a member of the union it reaches"
        );
        Some(owner)
    }

    /// The type an application's head denotes, where the load knows it closed.
    fn head_handle(&self, level: usize, head: &'graph ExpressionPart<'graph>) -> Option<KType> {
        match self.head_typed(level, head)? {
            Static::Closed(handle) => Some(handle),
            _ => None,
        }
    }

    /// The type an application's head denotes, as the load types it — closed, or rigid over the
    /// run-bound names it reads: a builtin type, a type binder, or a type expression. `None` for
    /// any other head.
    fn head_typed(
        &self,
        level: usize,
        head: &'graph ExpressionPart<'graph>,
    ) -> Option<StaticType<'o>> {
        let shape = self.chain[level].shape;
        match head {
            ExpressionPart::Type(_) => {
                let coordinate = shape.mention(shape.site(head))?.coordinate;
                if let Coordinate::Builtin(index) = coordinate {
                    return match self.builtins.get(index) {
                        Value::Type(value) => Some(Static::Closed(value.handle())),
                        _ => None,
                    };
                }
                let (at, slot) = self.landed_slot(level, coordinate)?;
                let holder = self.chain[at].shape;
                holder.declarations(slot)?;
                Some(self.declared_cell(holder, slot))
            }
            ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
                Some(self.type_cell(shape, shape.site(head)))
            }
            _ => None,
        }
    }

    /// Whether an application whose head is of the static type `typed` is read over names: its
    /// upper end a run-bound name, or a piece of one, whose bound guarantees no container, so a
    /// binding decides whether it reads, calls or neither.
    fn applies_over_names(&self, typed: Interval) -> bool {
        let (types, scratch) = (self.types, self.scratch);
        let upper = typed.upper;
        let piece = |step| judge::projection(types, scratch, upper, step).is_some();
        let container = piece(Step::Element)
            || (piece(Step::Key) && piece(Step::Value))
            || judge::rigid_within(types, scratch, upper, KType::STR)
            || judge::rigid_within(types, scratch, upper, KType::ANY_CODE);
        judge::node_unless_variable(types, upper).is_none()
            && judge::names_held(types, scratch, upper) == judge::Names::RunBound
            && !container
    }

    /// `(head argument)` over a head of the static type `typed`, written over run-bound names
    /// ([`applies_over_names`](Self::applies_over_names)): its ends the outputs of
    /// [`Rule::Application`], whose closed rule is the closed walk's read or call by name, so it is
    /// the same at load and under every binding. The argument is typed at the record a call wants
    /// ([`Rule::Wanted`]). Refused where a binding refuses it: the closed walk words the refusal.
    /// Routed, and porous, as a call of a callee the load does not know.
    fn applied_over_names(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        typed: Interval,
        argument: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let head = [DeclaredType::Type(typed.upper)];
        let wanted = judge::ruled_one(self, types, scratch, Rule::Wanted, &head);
        let payload = self.part_at(level, argument, wanted)?;
        let result = Parametric::from(super::builtins::result_type(self.builtins));
        let operands = [
            typed.lower,
            typed.upper,
            payload.lower,
            payload.upper,
            result,
        ]
        .map(DeclaredType::Type);
        if judge::defined(self, types, scratch, Rule::Application, &operands).0 == Some(false) {
            return Err(ShapeError::ReadNeverSatisfied {
                container: typed.upper,
                argument: payload.upper,
                at: node.source,
            });
        }
        let undecided = [KType::NEVER.into(), KType::ANY.into()];
        let ruled = judge::ruled(
            self,
            types,
            scratch,
            (Rule::Application, &operands),
            &undecided,
        );
        let applied = match ruled {
            Some([lower, upper]) => Interval {
                lower: *lower,
                upper: *upper,
            },
            _ => unknown(),
        };
        self.route_by_name(level, node, DeclaredType::Type(typed), Some(applied.upper));
        if porous_site(self.types, payload)
            && !self.holds_nothing_minted_part(level, argument, payload)?
        {
            self.exit(level);
        }
        Ok(applied)
    }

    /// `(head argument)` where the head denotes `head_type`, a type written over run-bound names:
    /// each end the construction of the type a binding gives the head over the payload's end
    /// ([`Rule::Construction`]), reached through the union a tag projection head's owner peels to,
    /// so it is the type a fresh load with the binding declared constructs.
    fn built_over_names(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        (head, head_type): (&'graph ExpressionPart<'graph>, Parametric),
        argument: &'graph ExpressionPart<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let payload = self.part(level, argument)?;
        let owner = match head {
            ExpressionPart::SigiledTypeExpr(node) => {
                tag_projection(node).and_then(|(owner, _)| self.declared(level, owner))
            }
            _ => None,
        };
        let end = |upper: bool, end: Parametric| {
            let mut operands = BumpVec::with_capacity_in(3, scratch);
            operands.extend([DeclaredType::Type(head_type), DeclaredType::Type(end)]);
            operands.extend(owner.map(DeclaredType::Type));
            let rule = Rule::Construction {
                upper,
                projected: owner.is_some(),
            };
            let undecided = Parametric::from(if upper { KType::ANY } else { KType::NEVER });
            judge::ruled(self, types, scratch, (rule, &operands), &[undecided])
                .map_or(undecided, |ended| ended[0])
        };
        let ends = Interval {
            lower: end(false, payload.lower),
            upper: end(true, payload.upper),
        };
        self.record_composite(
            level,
            self.node_site(level, node),
            [(argument, payload)].into_iter(),
        );
        Ok(ends)
    }

    /// A keyworded use: its arguments typed, each candidate judged against them — a builtin through
    /// its rule too, over what each slot holds as written — and the use narrowed, or a candidate
    /// selected, where no *maybe* one is left and one ranks first. Its static type is
    /// [`selection_type`](Self::selection_type)'s: each candidate's return on what it does at the
    /// call as every binding agrees, so it is the same at load and under every binding.
    fn narrow(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let mut uppers = BumpVec::new_in(self.scratch);
        let narrowed = self.narrow_arriving(level, node, list, &mut uppers)?;
        Ok(self.arriving(&uppers, narrowed))
    }

    /// [`narrow`](Self::narrow) where its arguments arrive, their upper ends pushed to `uppers`.
    fn narrow_arriving(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
        uppers: &mut BumpVec<'_, Parametric>,
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
            .position(self.node_site(level, node))
            .expect("a keyworded use's candidate list is recorded at its node");
        // A use one of whose arguments never arrives never runs its call.
        uppers.extend(arguments.iter().map(|argument| argument.upper));
        if self.never_arrives(uppers) {
            // Its lone candidate is the one the text selects, whose tree a call it never runs
            // keeps: a binding making an argument `Never` changes no tree.
            if let [Candidate::One(coordinate)] = list.candidates
                && !self.standing(level)
            {
                self.set_narrowing(level, index, Narrowing::Selected(*coordinate));
            }
            return Ok(Interval::point(KType::NEVER.into()));
        }
        // What each argument contributes where some candidate's slot solves from it: an instance
        // argument and a label their carried types. No other argument is read, so a type capture
        // is added only where a solve reads it.
        let mut solved = BumpVec::with_capacity_in(wanted.len(), scratch);
        solved.resize(wanted.len(), false);
        let mut reads = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        reads.extend(
            (list.candidates.iter()).map(|candidate| self.read_candidate(level, *candidate)),
        );
        let reads = reads.leak();
        for read in reads.iter() {
            for (each, solves) in solved.iter_mut().zip(read.solving) {
                *each |= *solves;
            }
        }
        let mut contributions = BumpVec::with_capacity_in(wanted.len(), scratch);
        // The positions whose upper end a solve reads, at every binding alike: a bit each.
        let mut pinned = 0u64;
        for (position, wanted) in wanted.iter().enumerate() {
            let instance = sites.iter().any(|site| site.position == position);
            let contribution = match wanted {
                Wanted::Evaluated(_) if solved[position] && !instance => {
                    self.contribution(level, arguments[position].upper, node.source)
                }
                _ => None,
            };
            if contribution.is_some() && position < 64 {
                pinned |= 1 << position;
            }
            contributions.push(contribution.unwrap_or(Static::Unknown));
        }
        let contributes = contributions
            .iter()
            .any(|contribution| !matches!(contribution, Static::Unknown));

        // Each argument as a refusal lists it: its upper end, or an instance argument's scheme.
        let lay = self.lay;
        let typed = |position: usize, argument: Interval| match sites
            .iter()
            .find(|site| site.position == position)
        {
            Some(site) => DeclaredType::Scheme(site.scheme),
            None => DeclaredType::Type(argument.upper),
        };
        let uppers = || {
            collect(
                lay,
                (arguments.iter().enumerate()).map(|(at, each)| typed(at, *each)),
            )
        };

        let mut judged = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        // The candidates every binding may run, as the choice of which does reads them, beside
        // each one's return; and whether one the load does not know is among them.
        let mut kept = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        let mut positions = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        let mut answered = false;
        let mut arms = BumpVec::with_capacity_in(list.candidates.len(), scratch);
        let mut unknown_kept = false;
        // The first argument a builtin's need dropped a candidate over, beside what it lacks.
        let mut dropped = None;
        // The last refusal of an instance argument a candidate was dropped over.
        let mut refused = None;
        for (position, (candidate, read)) in list.candidates.iter().zip(reads.iter()).enumerate() {
            // Unfilled code holds no function at an unmarked key's hole.
            if self.unfilled && self.hole(level, *candidate) {
                continue;
            }
            let known = read.known;
            // At this candidate, each instance argument is exactly the instance its slot makes.
            let (mut here, mut given_here) = (arguments, &given[..]);
            let mut instances: &[Made<'_>] = &[];
            if !sites.is_empty() {
                match self.instances(
                    known.shape(),
                    read.solving,
                    &sites,
                    arguments,
                    &contributions,
                ) {
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
            // A surfaced key's list is exactly its head where one head is the whole of the key: a
            // body definition, or a closed signature head, which every function a header or view
            // offers there is retyped to. Otherwise it may hold functions admitting more than the
            // head it is typed by, so it is at most *maybe*.
            let surfaced = read.surfaced;
            let exact = surfaced.is_some_and(|heads| match heads {
                [SurfacedHead::Body { .. }] => true,
                [SurfacedHead::Signature { .. }] => matches!(known, Known::Closed(_)),
                _ => false,
            });
            let surfacing = match surfaced {
                None => Surfacing::No,
                Some([SurfacedHead::Body { .. }]) => Surfacing::Body,
                Some([SurfacedHead::Signature { .. }]) => Surfacing::Signature,
                Some(_) => Surfacing::Loose,
            };
            let builtin = self.builtin(*candidate);
            // What every binding judges alike, beside what the walk judges, a witness's answer
            // among it: the call's type reads the first, its selection the second.
            let (mut verdict, intervals, running) = match known.shape() {
                Some(registered) => {
                    let solves = (read.solving.iter().enumerate())
                        .filter(|(position, solves)| **solves && *position < 64)
                        .fold(0u64, |mask, (position, _)| mask | 1 << position);
                    let running = Running {
                        shape: registered,
                        arguments: here,
                        pinned: pinned & solves,
                        builtin: builtin.is_some(),
                        one: matches!(candidate, Candidate::One(_)),
                        surfacing,
                    };
                    let (alike, _) = solve_read(types, scratch, &running);
                    let rigid = judge::judged(types, scratch, registered, alike);
                    if contributes {
                        here = self.solved_from(read.solving, here, &contributions);
                    }
                    let judged = match here == alike {
                        true => judge::witnessed_verdict(
                            self,
                            types,
                            scratch,
                            (registered, here),
                            rigid,
                        ),
                        false => judge::verdict(self, types, scratch, registered, here),
                    };
                    (
                        judged.verdict,
                        judged.solve.intervals(),
                        Some((running, rigid, alike)),
                    )
                }
                None => {
                    unknown_kept = true;
                    (Verdict::Maybe, None, None)
                }
            };
            let alike = running.map_or(Verdict::Maybe, |(_, rigid, _)| rigid.verdict);
            // A verdict a binding may give otherwise selects only through an answer.
            answered |= running.is_some() && verdict != alike;
            let mut ruled = None;
            if let Some(builtin) = builtin.filter(|_| alike != Verdict::Never) {
                let native = Native::of(builtin.id());
                let typed = rules::typed(self, native, builtin.ktype(), given_here, types, scratch);
                if native == Native::ModuleMember && verdict != Verdict::Never {
                    self.member_retype(level, node, given_here);
                }
                if let Some((slot, lacks)) = typed.dropped.filter(|_| verdict != Verdict::Never) {
                    verdict = Verdict::Never;
                    dropped = dropped.or(Some((given_here[slot].typed.lower, lacks)));
                }
                ruled = Some(typed.read);
            }
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
            // Each candidate some binding may run, beside what it returns where it runs.
            if let Some((running, rigid, read)) = running
                && (rigid.verdict != Verdict::Never
                    || surfacing == Surfacing::Loose
                    || surfacing == Surfacing::Signature && !retyped)
            {
                kept.push(running);
                positions.push(position as u16);
                arms.push(match ruled {
                    Some(ruled) => ruled,
                    None => {
                        let read = (running.shape, read);
                        let solved = judge::judged_intervals(self, types, scratch, read, rigid);
                        let surfaced = surfaced.is_some() && !retyped;
                        self.registered_return(running.shape, solved, surfaced)
                    }
                });
            }
            if verdict != Verdict::Never {
                judged.push(Judgement {
                    candidate: *candidate,
                    known,
                    surfaced: surfaced.is_some() && !retyped,
                    retyped,
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
                let (of, at) = (lower, node.source);
                Some(match lacks? {
                    Lacks::Field(field) => ShapeError::NoField { of, field, at },
                    Lacks::Member(member) => ShapeError::NoMember { of, member, at },
                })
            });
            // Each candidate with a shape names its refusal on a line of its own, worded where it
            // renders.
            let mut shapes = BumpVec::new_in(scratch);
            for (candidate, read) in list.candidates.iter().zip(reads.iter()) {
                let Some(shape) = read.known.shape() else {
                    continue;
                };
                if self.unfilled && self.hole(level, *candidate) {
                    continue;
                }
                shapes.push(shape);
            }
            return Err(missing.unwrap_or_else(|| ShapeError::NoAdmittingCandidate {
                key: list.elements,
                arguments: collect(lay, arguments.iter().copied()),
                listed: match sites.is_empty() {
                    true => &[],
                    false => uppers(),
                },
                shapes: collect(lay, shapes.into_iter()),
                at: node.source,
            }));
        }
        // The call's type, from which candidates run as every binding agrees.
        let ran = match unknown_kept {
            true => unknown(),
            false => self.selection_type(&kept, &arms),
        };
        // Where the text selects no one candidate, an answer selects among those kept, and the
        // use's content tree is keyed on the call.
        // Where the text selects no one candidate, an answer selects among those kept, and the
        // use's content tree is keyed on the call.
        let undecided = (!unknown_kept).then(|| {
            let undecided = crate::scope::UndecidedCall {
                kept: self.lay.fill(kept.len(), |at| kept[at].laid(self.lay)),
                positions: collect(self.lay, positions.iter().copied()),
                unfilled: self.unfilled,
            };
            resident(self.lay, undecided)
        });
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
                    && crate::scope::never_runs(types, scratch, maybe.into(), always()))
        }));
        let mut judged = left;

        // Where every candidate kept is one closed function, how they rank is the same at every
        // run: the table of each pair's answer, in scratch until a kept list records it. Over a
        // rigid one each pair is a question ([`judge::outranks`]), so a binding ranks it as a
        // fresh load does, and the table is laid where every pair is answered; else it is ranked
        // at the call. A spread — a surfaced key's, typed by its head — is ranked at the call
        // that knows its functions.
        let mut shapes: BumpVec<'_, DeclaredType<Parametric>> =
            BumpVec::with_capacity_in(judged.len(), scratch);
        shapes.extend(judged.iter().map_while(|judgement| {
            match (judgement.candidate, judgement.known) {
                (Candidate::One(_), Known::Closed(registered)) => {
                    Some(DeclaredType::from(registered))
                }
                (Candidate::One(_), Known::Rigid(shape)) => Some(shape),
                _ => None,
            }
        }));
        let ranks = shapes.len() == judged.len();
        let closed =
            ranks && (judged.iter()).all(|judgement| matches!(judgement.known, Known::Closed(_)));
        let builtin = |at: usize| self.builtin(judged[at].candidate).is_some();
        // A lone candidate ranks against nothing.
        let table = match (ranks && judged.len() > 1, closed) {
            (false, _) => None,
            (true, true) => select::table(scratch, types, scratch, &shapes, builtin),
            (true, false) => {
                let mut asked = |class, a: usize, b: usize| {
                    judge::outranks(self, types, scratch, (shapes[a], shapes[b]), class)
                };
                crate::scope::Table::asked(scratch, types, scratch, &shapes, builtin, &mut asked)
            }
        };
        if judged
            .iter()
            .all(|judgement| judgement.verdict == Verdict::Always)
        {
            let decided = match (&judged[..], table) {
                ([_], _) => Some(Ok(0)),
                (_, Some(table)) => Some(table.decide(u64::MAX >> (64 - judged.len()))),
                // More than a table ranks: the lattice's own elimination.
                _ if closed => {
                    let survivors = judge::select(types, scratch, &shapes);
                    Some(crate::scope::winner(&survivors, builtin))
                }
                // Several, a rigid one among them: ranked at the call.
                _ => None,
            };
            let chosen = match decided {
                Some(Ok(survivor)) => Some(judged[survivor]),
                Some(Err(count)) => {
                    return Err(ShapeError::Ambiguous {
                        key: list.elements,
                        arguments: uppers(),
                        count,
                        at: node.source,
                    });
                }
                None => None,
            };
            // A candidate whose group has a variable with no point is left to solve at the call.
            let chosen = chosen.and_then(|chosen| {
                let points = self.points(level, &chosen, &contributions, node.source)?;
                Some((chosen, points))
            });
            if let Some((chosen, points)) = chosen {
                let Candidate::One(coordinate) = chosen.candidate else {
                    unreachable!("an always candidate is no spread");
                };
                self.use_exits(level, arguments, minted_free, &sites, &[chosen]);
                self.made(level, node, list, &sites, &[chosen])?;
                let site = self.node_site(level, node);
                if !self.unfilled {
                    self.record_selection(level, site, points);
                }
                self.set_narrowing(level, index, Narrowing::Selected(coordinate));
                if let Some(undecided) = undecided.filter(|_| answered) {
                    self.set_undecided(level, index, undecided);
                }
                if !self.unfilled && self.store.traces() {
                    self.trace(level, node, coordinate, site);
                }
                self.route_use(level, node, &[(chosen, Some(points))]);
                return Ok(ran);
            }
        }
        // An *always* candidate whose group has a variable with no point is admitted at the call.
        let mut points = BumpVec::with_capacity_in(judged.len(), scratch);
        for judgement in judged.iter_mut() {
            let solved = match judgement.verdict {
                Verdict::Always => self.points(level, judgement, &contributions, node.source),
                _ => Some(Points::NONE),
            };
            points.push(solved.unwrap_or(Points::NONE));
            if solved.is_none() {
                judgement.verdict = Verdict::Maybe;
            }
        }
        if judged.len() < list.candidates.len()
            || table.is_some()
            || judged
                .iter()
                .any(|judgement| judgement.verdict == Verdict::Always)
        {
            let entries = collect(
                self.lay,
                judged
                    .iter()
                    .map(|judgement| (judgement.candidate, judgement.verdict)),
            );
            let points = collect(self.lay, points.iter().copied());
            let elimination = match table {
                Some(table) => Elimination::Table(table.relaid(self.lay)),
                None => Elimination::Open,
            };
            let kept = resident(
                self.lay,
                Kept {
                    entries,
                    points,
                    elimination,
                },
            );
            self.set_narrowing(level, index, Narrowing::Kept(kept));
        }
        if let Some(undecided) = undecided {
            self.set_undecided(level, index, undecided);
        }
        let mut routed = BumpVec::with_capacity_in(judged.len(), scratch);
        routed.extend(
            (judged.iter().zip(points.iter())).map(|(judgement, points)| {
                let solved = judgement.verdict == Verdict::Always;
                (*judgement, solved.then_some(*points))
            }),
        );
        self.route_use(level, node, &routed);
        self.use_exits(level, arguments, minted_free, &sites, &judged);
        self.made(level, node, list, &sites, &judged)?;
        // A lone closed signature head, with no hole skipped, is the only list that may admit, and
        // every function in it returns retyped to the head's return.
        Ok(ran)
    }

    /// The type of a call whose candidates every binding may run are `kept`, each returning its
    /// `arms` entry where it runs: the union, per end, of each candidate's return on what it does
    /// ([`Run`]) — closed where every binding agrees, else a decided name per candidate on
    /// [`Question::Runs`], so a binding gives what a fresh load selects.
    fn selection_type(&self, kept: &[Running<'_>], arms: &[Interval]) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let never = Parametric::from(KType::NEVER);
        let (mut lowers, mut uppers) = (
            BumpVec::with_capacity_in(kept.len(), scratch),
            BumpVec::with_capacity_in(kept.len(), scratch),
        );
        match runs(types, scratch, kept, self.unfilled) {
            Some(answers) => {
                for (answer, arm) in answers.iter().zip(arms) {
                    let (lower, upper) = match answer {
                        Run::Never => (never, never),
                        Run::May => (never, arm.upper),
                        Run::Selected => (arm.lower, arm.upper),
                    };
                    lowers.push(lower);
                    uppers.push(upper);
                }
            }
            None => {
                let kept = collect(scratch, kept.iter().copied());
                for (candidate, arm) in arms.iter().enumerate() {
                    let question = Question::Runs {
                        kept,
                        candidate: candidate as u16,
                        unfilled: self.unfilled,
                    };
                    let decided = |arms: &[Parametric], undecided| {
                        judge::decided(self, types, scratch, (question, arms), undecided)
                    };
                    lowers.push(decided(&[never, never, arm.lower], never));
                    uppers.push(decided(&[never, arm.upper, arm.upper], arm.upper));
                }
            }
        }
        Interval {
            lower: types.union_of(scratch, &lowers),
            upper: types.union_of(scratch, &uppers),
        }
    }

    /// Record what the member read `node`, given `given`, re-stamps the member at, where its module
    /// operand is exact: the run reads the member there, at its declared type written over names,
    /// re-stamped as the [restamp](judge::restamp) question answers.
    fn member_retype(&mut self, level: usize, node: &KExpression<'graph>, given: &[Given<'_>]) {
        let (types, scratch) = (self.types, self.scratch);
        let [module, label] = given else {
            return;
        };
        let Some([name @ BinderSymbol::Value(_)]) = label.names else {
            return;
        };
        if !judge::exact(self, types, module.typed) {
            return;
        }
        let restamp = signature_member(self, types, scratch, module.typed.upper, *name)
            .and_then(|read| read.declared.as_type())
            .and_then(|declared| judge::restamp(self, types, scratch, declared));
        if let Some(restamp) = restamp
            && !self.unfilled
        {
            self.record_retype(level, self.node_site(level, node), restamp);
        }
    }

    /// The return of the callable body whose tail call `node` is, as the contract its frame owes
    /// holds it, written over names: `node` the last statement's call of the body at `level`, the
    /// callable or a tail arm's block in it. `Some(None)` where that return is open.
    fn tail_returns(&self, level: usize, node: &KExpression<'graph>) -> Option<Option<Parametric>> {
        let shape = self.chain[level].shape;
        let owner = self.shapes.tail_owner(shape)?;
        if self.unfilled || owner.kind() != ShapeKind::Callable {
            return None;
        }
        let tail = self.shapes.tail_call(shape)?;
        if !std::ptr::eq(tail, node) {
            return None;
        }
        let owned = (0..=level)
            .rev()
            .find(|at| self.chain[*at].shape.segment() == owner.segment())?;
        Some(self.in_body(owned, Part::Return))
    }

    /// Whether a call of `callee`, its group at `solution`, keeps the contract `returns`, all
    /// written over names: [`kept`](crate::program::kept)'s rule, its return's fit decided where a
    /// binding does not decide it ([`judge::fit_decided`]). With no solution a quantified callee
    /// keeps nothing.
    fn kept_decided(
        &self,
        returns: Parametric,
        callee: DeclaredType<Parametric>,
        solution: Option<&[Parametric]>,
    ) -> Option<bool> {
        let (types, scratch) = (self.types, self.scratch);
        let solution = match (callee, solution) {
            (DeclaredType::Type(_), _) => &[][..],
            (DeclaredType::Scheme(_), Some(solution)) => solution,
            (DeclaredType::Scheme(_), None) => return Some(false),
        };
        let (TypeNode::KFunction { ret, .. } | TypeNode::ExpressionShape { ret, .. }) =
            callee.view(types).body
        else {
            return Some(false);
        };
        let ret = crate::type_lattice::substitute_quantified(types, scratch, ret, solution);
        judge::fit_decided(self, types, scratch, ret, returns)
    }

    /// Record the route of the keyworded use `node` where it is the tail call of the callable body
    /// at `level`: per candidate the call reads, in its order — each judged, beside the closed
    /// points an *always* one's group solves to — whether its return keeps the frame's contract,
    /// where the frame's return and its solution are closed, and whether it hops into the frame's
    /// own body. A builtin's call, a surfaced head's and a spread's are decided at the call.
    fn route_use(
        &mut self,
        level: usize,
        node: &KExpression<'graph>,
        judged: &[(Judgement<'_>, Option<Points<'_>>)],
    ) {
        let Some(returns) = self.tail_returns(level, node) else {
            return;
        };
        let (shape, scratch) = (self.chain[level].shape, self.scratch);
        let per = self.lay.fill(judged.len(), |at| {
            let (judgement, points) = judged[at];
            let Candidate::One(coordinate) = judgement.candidate else {
                return CandidateRoute {
                    kept: None,
                    hop: false,
                };
            };
            let decided = judgement.ruled.is_none() && !judgement.surfaced && !judgement.retyped;
            let kept = match (returns, judgement.known) {
                (Some(returns), Known::Closed(callee)) if decided => {
                    let solution = match callee {
                        DeclaredType::Type(_) => Some(BumpVec::new_in(scratch)),
                        DeclaredType::Scheme(_) => points.and_then(|points| {
                            let mut solved =
                                BumpVec::with_capacity_in(points.points.len(), scratch);
                            for at in 0..points.points.len() {
                                solved.push(Parametric::from(points.closed(at)?));
                            }
                            Some(solved)
                        }),
                    };
                    solution.and_then(|solution| {
                        self.kept_decided(returns, callee.into(), Some(&solution))
                    })
                }
                _ => None,
            };
            CandidateRoute {
                kept,
                hop: self.shapes.hops_at(shape, coordinate),
            }
        });
        self.record_route(level, self.node_site(level, node), Route { per });
    }

    /// Record the route of the call by name `node` of a callee whose static type is `head` where it
    /// is the tail call of the callable body at `level`: whether an exact callee's return keeps the
    /// frame's contract, and whether it hops into the frame's own body. A variable head's call
    /// keeps it where its result's upper end `over_names`, the output of the rule that types the
    /// call ([`applied_over_names`](Self::applied_over_names)), fits the contract: the function's
    /// return wherever the call runs. A variable head no rule types leaves it to the run.
    fn route_by_name(
        &mut self,
        level: usize,
        node: &KExpression<'graph>,
        head: Bound,
        over_names: Option<Parametric>,
    ) {
        let Some(returns) = self.tail_returns(level, node) else {
            return;
        };
        let (shape, types, scratch) = (self.chain[level].shape, self.types, self.scratch);
        // An exact callee over names is read as written.
        let callee = match head {
            DeclaredType::Type(head) => match judge::node_unless_variable(types, head.upper) {
                Some(_) => {
                    judge::exact(self, types, head).then_some(DeclaredType::Type(head.upper))
                }
                None => None,
            },
            DeclaredType::Scheme(scheme) => Some(DeclaredType::Scheme(scheme)),
        };
        let kept = match (returns, callee, over_names) {
            (Some(returns), Some(callee), _) => self.kept_decided(returns, callee, None),
            (Some(returns), None, Some(upper)) => {
                judge::fit_decided(self, types, scratch, upper, returns)
            }
            _ => None,
        };
        let hop = self.shapes.hops_by_name(shape, &node.parts[0].value);
        let per = self.lay.fill(1, |_| CandidateRoute { kept, hop });
        self.record_route(level, self.node_site(level, node), Route { per });
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
        // One scratch copy of the arguments, patched per candidate with its instances.
        let mut here = BumpVec::with_capacity_in(arguments.len(), self.scratch);
        let exits = kept.iter().any(|judgement| {
            if self.builtin(judgement.candidate).is_some() {
                return false;
            }
            here.clear();
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
        let exits = |slots: &mut dyn Iterator<Item = Parametric>| {
            (slots.zip(arguments).zip(minted_free)).any(|((slot, typed), free)| {
                !free && porous_site(types, *typed) && porous_slot(types, slot)
            })
        };
        match shape {
            DeclaredType::Type(shape) => exits(&mut shape_slots(shape, types)),
            DeclaredType::Scheme(scheme) => exits(&mut scheme_slots(scheme, types)),
        }
    }

    /// Whether `part` is a value the frame body at `level` received — a parameter of that body, a
    /// capture, a hole or a name offered, or a `LET` bound bare to one, named bare, or a member read
    /// of one through the builtin `ATTR` alone — which cannot hold what the body mints, so passing
    /// it on is no exit
    /// ([type tiers](../../design/type-tiers.md#two-classes-of-body)). Read once per part, through
    /// the level's memo.
    fn received(&mut self, level: usize, part: &'graph ExpressionPart<'graph>) -> bool {
        let site = self.site(level, part);
        if let Some(received) = self.chain[level].received.get(&site) {
            return *received;
        }
        #[cfg(test)]
        count(|counts| counts.receipts += 1);
        let received = self.received_form(level, of_part(self.chain[level].shape, part));
        self.chain[level].received.insert(site, received);
        received
    }

    /// [`received`](Self::received) over what the evaluator reads as `form`.
    fn received_form(&mut self, level: usize, form: Form<'graph>) -> bool {
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
        let Some(mention) = shape.mention(shape.site(name)) else {
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
    fn received_binder(&mut self, at: usize, slot: Slot, frame: usize) -> bool {
        let shape = self.chain[at].shape;
        if at < frame || (at == frame && shape.position_of(slot) == Position::PARAMETER) {
            return true;
        }
        shape.rhs(slot).is_some_and(|rhs| {
            rhs.is_name()
                && shape
                    .mention(shape.site(rhs))
                    .is_some_and(|mention| !mention.fellow)
                && self.received(at, rhs)
        })
    }

    /// The static type the pass recorded for `part` of the shape at `level`, where it typed it.
    fn typed_part(&self, level: usize, part: &'graph ExpressionPart<'graph>) -> Option<Interval> {
        self.part_type(level, self.site(level, part))
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
        if judge::kind_above(self, types, scratch, typed.upper) != KKind::Signature {
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
        self.holds_nothing_minted(level, form, Written::Part(self.site(level, part)), typed)
    }

    /// Whether the content tree `tree`, read in the shape at `level`, reaches content its frame
    /// body minted: a view, open or closed, since a closed one may still mint past the tree's
    /// horizon; a closure, whose type or captures may name it; a module body holding a minting
    /// site, in itself or in a block or module body it runs, or capturing what reaches it; or a
    /// leaf on anything but a value the frame body received. A leaf reading a member, or keyed on
    /// an unfolded call, reaches it too: the tree does not say the member read was the builtin's.
    fn reaches_minted(&mut self, level: usize, tree: &Tree<'o>) -> bool {
        let frame = self.frame_level(level);
        let mut stack = BumpVec::new_in(self.scratch);
        stack.push(*tree);
        while let Some(tree) = stack.pop() {
            match tree {
                Tree::Digest(..) => {}
                // A type over names may name what the frame minted, by a path through it; over
                // run-bound names the frame received alone, it names nothing minted.
                Tree::TypeValue { named, .. } => {
                    if !self.names_received(level, frame, named) {
                        return true;
                    }
                }
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
                Tree::View { .. }
                | Tree::Knot { .. }
                | Tree::Leaf(Leaf::Unnamed(_) | Leaf::Call { .. }) => {
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

    /// Whether `named`, a type value written over names read in the shape at `level`, holds only
    /// lexical variables, each homed at a binder the frame body at `frame` received.
    fn names_received(&mut self, level: usize, frame: usize, named: Parametric) -> bool {
        let mut levels = BumpVec::new_in(self.scratch);
        let mut other = false;
        judge::read_points(
            self.types,
            self.scratch,
            named,
            Side::Above,
            &mut |variable| {
                match variable {
                    Variable::Lexical { level, .. } => levels.push(level),
                    _ => other = true,
                }
                None
            },
        );
        if other {
            return false;
        }
        levels.into_iter().all(|variable| {
            let home = (0..=level).rev().find_map(|at| {
                match self.shapes.home(self.chain[at].shape.segment(), variable)? {
                    crate::scope::Target::Local(slot) => Some((at, slot)),
                    crate::scope::Target::Capture(_) => None,
                }
            });
            home.is_some_and(|(at, slot)| self.received_binder(at, slot, frame))
        })
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
        self.record_part(level, self.site(level, site.part), typed);
        if self.unfilled {
            return;
        }
        let solution = self.instance(level, solution, typed.upper, site.at);
        let read = InstanceRead::Solved(solution);
        match site.instanced {
            Instanced::Name(leaf) => self.record_instance_read(level, self.site(level, leaf), read),
            Instanced::Literal(body) => self.fix_born_instance(body, solution),
            Instanced::Member(node) => {
                self.record_instance_read(level, self.node_site(level, node), read)
            }
        }
    }

    /// What a call of the registered shape `registered` returns, its group solved to `intervals`:
    /// its return read through them — exactly that return where the retype makes it so and the
    /// solve is the call's, the shape unquantified or every interval a point, since its frame
    /// retypes its value to it, and at most it otherwise. A `surfaced` head's return is at most
    /// whatever the solve: the module's own definition answers the call, and the head states only
    /// a bound on what it returns — save a closed signature head, which the call's frame retypes
    /// to under the selection's solution, and which the caller passes as not `surfaced`. The lower
    /// end is the type on the solve's exactness, the same at load and under every binding.
    fn registered_return(
        &self,
        registered: DeclaredType<Parametric>,
        intervals: Option<&[Interval]>,
        surfaced: bool,
    ) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let returned = self.returned(registered, intervals);
        let solved = match (surfaced, registered, intervals) {
            (true, _, _) | (_, DeclaredType::Scheme(_), None) => return under(returned),
            (_, DeclaredType::Type(_), _) => &[][..],
            (_, DeclaredType::Scheme(_), Some(intervals)) => intervals,
        };
        // Exactly the return where every interval of the solve is exact: a decided lower end.
        let mut exact = BumpVec::with_capacity_in(solved.len(), scratch);
        exact.extend(solved.iter().map(|each| crate::scope::Question::Same {
            a: each.lower.into(),
            b: each.upper.into(),
        }));
        let mut retyped = || judge::retyped(self, types, scratch, returned).lower;
        let lower = judge::when_all(
            self,
            types,
            scratch,
            &exact,
            &mut retyped,
            KType::NEVER.into(),
        );
        Interval {
            lower,
            upper: returned,
        }
    }

    /// The type of the code the part `operand` runs, where the load traces it to a written quote:
    /// its last statement's upper end as it runs unfilled, over the names of the chain it lies on —
    /// traced code is written, so nothing composed or `USING` filled it.
    fn traced(&self, level: usize, operand: &'graph ExpressionPart<'graph>) -> Option<Parametric> {
        let code = self.traced_code(level, operand)?;
        Some(self.ran_of(code)?.upper)
    }

    /// The code shape the part `operand` runs, where the load traces it to a written quote: the
    /// quote itself, or a name a `LET` binds to one, by [`traced_quote`]'s rule, which a callable's
    /// named body shares. Composed code has no shape at load.
    fn traced_code(
        &self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
    ) -> Option<&'graph BodyShape<'graph>> {
        let shape = self.chain[level].shape;
        let traced = match operand {
            ExpressionPart::QuotedExpression(_) => shape.nested(shape.site(operand)),
            ExpressionPart::Identifier(_) => {
                let coordinate = shape.mention(shape.site(operand))?.coordinate;
                let (at, slot) = self.landed_slot(level, coordinate)?;
                let holder = self.chain[at].shape;
                let rhs = holder.rhs(slot)?;
                traced_quote(rhs)?;
                holder.nested(holder.site(rhs))
            }
            _ => None,
        };
        // Code composed where it is born has no shape at load.
        traced.filter(|code| !code.composing())
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

    /// What the load knows of `candidate`'s registered shape, read from the shape at `level`, where
    /// `surfaced` is the heads `candidate` holds where it is a surfaced key.
    fn candidate(
        &self,
        level: usize,
        candidate: Candidate,
        surfaced: Option<&'graph [SurfacedHead<'graph>]>,
    ) -> Known {
        #[cfg(test)]
        count(|counts| counts.candidate_reads += 1);
        let coordinate = match candidate {
            // A surfaced key's list is typed by its head; any other spread is unknown.
            Candidate::Spread(coordinate) if surfaced.is_some() => coordinate,
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
        match self.resolved_registered(typed) {
            Static::Closed(registered) => Known::Closed(registered.shape),
            Static::Rigid { value, .. } => Known::Rigid(value.shape),
            Static::Unknown => Known::Unknown,
        }
    }

    /// What a keyworded use at the shape at `level` reads of `candidate`.
    fn read_candidate(&self, level: usize, candidate: Candidate) -> CandidateRead<'p, 'graph> {
        let surfaced = self.surfaced(level, candidate);
        let known = self.candidate(level, candidate, surfaced);
        let solving = match known.shape() {
            Some(shape) => self.solving(shape),
            None => &[],
        };
        CandidateRead {
            known,
            surfaced,
            solving,
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
        ExpressionPart::QuotedExpression(node) => one_name(node),
        _ => None,
    };
    let mut names = BumpVec::new_in(scratch);
    match part {
        ExpressionPart::ListLiteral(items) => {
            for item in items.iter() {
                names.push(quoted(item.as_part()?)?);
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

/// The static type of a list literal whose elements' static types are `elements`: a list over
/// [`filled_ends`].
fn list_interval(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    elements: &[Interval],
) -> Interval {
    let (lower, upper) = filled_ends(marks, types, scratch, elements, &[]);
    Interval {
        lower: types.list(lower),
        upper: types.list(upper),
    }
}

/// The ends of a container literal's element type over `filled`, its elements' static types, and
/// `gates`, the static types of what else must be exact for the run to fill it — a dict's keys:
/// [`Rule::Fill`](crate::scope::Rule::Fill) through its door, so the run's fill where every one
/// is exact and the plain ends otherwise are one type at load and under every binding. Outside a
/// walk, `Never` up to the union of the uppers, which the fill only narrows.
fn filled_ends(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    filled: &[Interval],
    gates: &[Interval],
) -> (Parametric, Parametric) {
    let mut operands = BumpVec::with_capacity_in(2 * (filled.len() + gates.len()), scratch);
    for each in filled.iter().chain(gates) {
        operands.push(DeclaredType::Type(each.lower));
        operands.push(DeclaredType::Type(each.upper));
    }
    let mut uppers = BumpVec::with_capacity_in(filled.len(), scratch);
    uppers.extend(filled.iter().map(|each| each.upper));
    let undecided = [KType::NEVER.into(), types.union_of(scratch, &uppers)];
    let rule = crate::scope::Rule::Fill {
        filled: filled.len() as u32,
    };
    match judge::ruled(marks, types, scratch, (rule, &operands), &undecided) {
        Some([lower, upper]) => (*lower, *upper),
        _ => (undecided[0], undecided[1]),
    }
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

/// What a load did, counted for the tests that bound it: each a kind of work the pass's memos keep
/// linear in the program.
#[cfg(test)]
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct LoadCounts {
    /// Reads of what a view binds ([`view_of`](Pass::view_of)).
    pub view_reads: usize,
    /// Fits the walk computed for a `:|`'s view.
    pub fits: usize,
    /// Reads of the type a view's source carries, made to fit it.
    pub source_reads: usize,
    /// Labels laid in program storage for leaves' chains.
    pub chain_labels: usize,
    /// Slices laid in program storage for bodies' captures.
    pub capture_slices: usize,
    /// Entries laid in frontiers: each unknown a unit gathered itself, and each frontier it read.
    pub frontier_entries: usize,
    /// Entries the walk's gatherings took: an unknown, or a read of an ended frontier.
    pub noted: usize,
    /// Reads of what the load knows of a candidate's registered shape
    /// ([`candidate`](Pass::candidate)).
    pub candidate_reads: usize,
    /// Reads of the slots a shape's solve reads ([`solving`](Pass::solving)).
    pub solving_reads: usize,
    /// Peeks computed at a part ([`peeked`](Pass::peeked)), past the level's memo.
    pub peeks: usize,
    /// Reads of whether a part is a value its frame body received
    /// ([`received`](Pass::received)), past the level's memo.
    pub receipts: usize,
    /// Reads a rooted walk made of a standing level's records
    /// ([`stood_record`](Pass::stood_record)).
    pub stood: usize,
}

#[cfg(test)]
thread_local! {
    /// How many units a resumption re-ran, by the kind of open mark on a bound name each reaches:
    /// a unit counted once per kind it holds, and at [`UNMARKED`] where it reaches none. Tests
    /// reset it.
    pub(crate) static RERUNS: std::cell::Cell<[usize; UNMARKED + 1]> =
        const { std::cell::Cell::new([0; UNMARKED + 1]) };
    /// [`RERUNS`] for the witnessing walks alone, by the kind of open mark on a bound or witnessed
    /// name. Tests reset it.
    pub(crate) static WITNESSED: std::cell::Cell<[usize; UNMARKED + 1]> =
        const { std::cell::Cell::new([0; UNMARKED + 1]) };
    /// How many questions a witnessing walk's doors had answered. Tests reset it.
    pub(crate) static QUESTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The open marks each unit record a walk laid holds of its own, by kind: the records holding
    /// one of the kind, its answered marks, its undecided ones. Tests reset it.
    pub(crate) static TALLY: std::cell::Cell<[Tallied; UNMARKED]> =
        const { std::cell::Cell::new([Tallied { records: 0, answered: 0, undecided: 0 }; UNMARKED]) };
    /// How many units a closing walk over a witnessed layer re-ran for an undecided mark on a name
    /// it checks: the units that held through the witnessing walk. Tests reset it.
    pub(crate) static HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One kind's row of [`TALLY`].
#[cfg(test)]
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Tallied {
    pub records: usize,
    pub answered: usize,
    pub undecided: usize,
}

/// Count the open marks of `frontier`'s own in [`TALLY`].
#[cfg(test)]
fn tally(frontier: Frontier<'_>) {
    TALLY.with(|tally| {
        let mut rows = tally.get();
        let mut seen = [false; UNMARKED];
        for noted in frontier.own() {
            if let crate::scope::Mark::Open(open) = noted.mark {
                let row = &mut rows[open.kind as usize];
                match open.answered {
                    Some(_) => row.answered += 1,
                    None => row.undecided += 1,
                }
                seen[open.kind as usize] = true;
            }
        }
        (rows.iter_mut().zip(seen)).for_each(|(row, seen)| row.records += usize::from(seen));
        tally.set(rows);
    });
}

/// The slot of [`RERUNS`] counting units that reach no open mark on a bound or witnessed name.
#[cfg(test)]
pub(crate) const UNMARKED: usize = crate::scope::OpenKind::ModuleContent as usize + 1;

#[cfg(test)]
impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// Count a re-run of a record of frontier `frontier` in [`RERUNS`], or a witnessing walk's in
    /// [`WITNESSED`].
    fn count_rerun(&self, frontier: Frontier<'o>) {
        if self.store.held(frontier) {
            HELD.with(|held| held.set(held.get() + 1));
        }
        let mut witnessed = BumpVec::new_in(self.scratch);
        (self.store).each_witnessed(&mut |unknown| witnessed.push(unknown));
        let mut kinds = [false; UNMARKED + 1];
        for noted in frontier.marks() {
            if let crate::scope::Mark::Open(open) = noted.mark
                && (self.store.binding(noted.unknown).is_some()
                    || witnessed.contains(&noted.unknown))
            {
                kinds[open.kind as usize] = true;
            }
        }
        if !kinds.contains(&true) {
            kinds[UNMARKED] = true;
        }
        let counter = match witnessed.is_empty() {
            true => &RERUNS,
            false => &WITNESSED,
        };
        counter.with(|reruns| {
            let mut counts = reruns.get();
            (counts.iter_mut().zip(kinds)).for_each(|(count, held)| *count += usize::from(held));
            reruns.set(counts);
        });
    }
}

#[cfg(test)]
thread_local! {
    /// What the loads on this thread did since [`counted`] last began.
    static COUNTS: std::cell::Cell<LoadCounts> = std::cell::Cell::new(LoadCounts::default());
    /// What each unit the walks on this thread run or absorb in their own order
    /// ([`unit`](Pass::unit)) is handed to, while [`visiting`] runs: not the chain a rooted walk
    /// stands.
    static VISIT: std::cell::Cell<Option<fn(UnitId)>> = const { std::cell::Cell::new(None) };
}

/// Run `walk`, handing `visit` each unit it runs or absorbs in its own order.
#[cfg(test)]
pub(super) fn visiting<R>(visit: fn(UnitId), walk: impl FnOnce() -> R) -> R {
    VISIT.with(|held| held.set(Some(visit)));
    let read = walk();
    VISIT.with(|held| held.set(None));
    read
}

/// Add to one of the load's counts.
#[cfg(test)]
fn count(bump: impl FnOnce(&mut LoadCounts)) {
    COUNTS.with(|counts| {
        let mut now = counts.get();
        bump(&mut now);
        counts.set(now);
    });
}

/// Run `load` from zero counts, beside what it counted.
#[cfg(test)]
pub(super) fn counted<R>(load: impl FnOnce() -> R) -> (R, LoadCounts) {
    COUNTS.with(|counts| counts.set(LoadCounts::default()));
    let read = load();
    (read, COUNTS.with(std::cell::Cell::get))
}

/// Whether loads leave every keyworded use whole now.
fn unnarrowed_now() -> bool {
    #[cfg(test)]
    return UNNARROWED.with(std::cell::Cell::get);
    #[cfg(not(test))]
    false
}

/// Each splice the quote `node` owns, its operand beside where it is written: among its own parts,
/// its groups', each literal element's, and those of each quote a builtin reads as written.
fn each_splice(node: &KExpression<'_>, out: &mut dyn FnMut(ValueSymbol, SourceRef)) {
    let slots = node.written_slots();
    for (index, part) in node.parts.iter().enumerate() {
        match &part.value {
            ExpressionPart::Splice(name) => out(
                *name,
                SourceRef {
                    span: part.span.unwrap_or(node.source.span),
                    ..node.source
                },
            ),
            part => each_splice_under(part, slots.at(index), out),
        }
    }
}

/// Each splice the quote around `part` owns under it, `written` whether its slot is read as written:
/// in a group, a quote a builtin reads as written, or a literal's elements, read as their literal is.
fn each_splice_under(
    part: &ExpressionPart<'_>,
    written: bool,
    out: &mut dyn FnMut(ValueSymbol, SourceRef),
) {
    match part {
        ExpressionPart::Expression(inner) => each_splice(inner, out),
        ExpressionPart::QuotedExpression(inner) if written => each_splice(inner, out),
        literal if literal.is_literal() => {
            let mut at = 0;
            while let Some(element) = literal.child(at) {
                each_splice_under(element, written, out);
                at += 1;
            }
        }
        _ => {}
    }
}
