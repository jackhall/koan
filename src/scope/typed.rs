//! The vocabulary of load-time types: what [the elaborator's load pass](../elaborate/README.md#the-type-channel-at-load)
//! fixes for a shape before the program runs, and the records it fixes.
//!
//! `scope` sits below `elaborate`, so the records the elaborator hands back for a callable — its
//! [`Callable`] type, the [`Registered`] shape its bucket holds, where each `FOR ALL` name landed
//! in its group — and its refusal ([`Elaboration`]) live here, beside the shape whose write-once
//! cells hold them. A cell holds a [`Static`]: unknown, closed — a concrete value, the same at
//! every run — or rigid over [`Variable`]s — a parametric value over the lexical variables the run
//! supplies, by level — which [`solutions`] reads through an activation for one substitution.
//!
//! The value channel's records live here too: the [`Statics`] the language's load pass fixes for a
//! body, the [`UseStatics`] of each keyworded use: its candidates' [`Narrowing`] and its
//! arguments' contributions, and a frame body's [`BodyClass`].
//!
//! See [README.md § Load-time types](README.md#load-time-types).

use std::cell::Cell;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::memory::{BumpAllocator, BumpHashTable, BumpVec, Writer, collect};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Verdict, display_name};
use crate::values::{Chain, KnottedFamily, Value};

use super::activation::ActivationView;
use super::keyed::Keyed;
use super::shape::{Candidate, Coordinate, Site, Slot};
use super::tree::{Binder, Tree};

/// A callable's type, and how its `FOR ALL` group's declaration order maps onto that type's
/// group — what a call needs to bind each type parameter to its solution. `T` is what the type is
/// over where no group binds: a [`KType`] for a callable the load fixed, a [`Parametric`] for one
/// over the run's variables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Callable<'x, T = Parametric> {
    /// The function type, or a scheme where a `FOR ALL` group quantifies it.
    pub ktype: DeclaredType<T>,
    /// Each `FOR ALL` name the declaration wrote, with its index in `ktype`'s group.
    pub quantifier_map: FunctionGroupMap<'x>,
    /// What the registration the callable is born for puts in its bucket; `None` for a callable no
    /// registration binds — a `FN`, or a combined statement's name.
    pub registered: Option<Registered<'x, T>>,
}

/// What a registration's bucket holds of the function it binds, and what a keyworded call of it
/// reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registered<'x, T = Parametric> {
    /// The expression shape: the function type's parameters laid over the registration's key,
    /// ranked by the registration's classes; a scheme where a `FOR ALL` group quantifies it.
    pub shape: DeclaredType<T>,
    /// Each `FOR ALL` name the declaration wrote, with its index in `shape`'s group.
    pub quantifier_map: ShapeGroupMap<'x>,
    pub parameters: ParameterBinding<'x>,
}

/// Each `FOR ALL` name a declaration wrote, in written order, with its index in its callable's
/// **function type**'s group — which numbers the variables by first occurrence in symbol-sorted
/// parameter order. Empty for an unquantified callable.
///
/// The **name** is the key, not the position: a callee's type-parameter slots reach its frame
/// symbol-sorted, not in written order, and the type's own `quantifiers` cannot stand in for this
/// because alpha-variants intern to one node and it holds whichever spelling interned first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FunctionGroupMap<'x>(pub &'x [(TypeSymbol, usize)]);

/// Each `FOR ALL` name a declaration wrote, in written order, with its index in its registered
/// **shape**'s group — which numbers the variables by first occurrence in element order, not as
/// the function type's group does. Empty for an unquantified registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ShapeGroupMap<'x>(pub &'x [(TypeSymbol, usize)]);

macro_rules! group_map {
    ($($map:ident),*) => {$(
        impl<'x> $map<'x> {
            pub fn is_empty(self) -> bool {
                self.0.is_empty()
            }

            /// Each name with its group index, in written order.
            pub fn iter(self) -> impl ExactSizeIterator<Item = (TypeSymbol, usize)> + 'x {
                self.0.iter().copied()
            }

            /// The group index the name `name` landed at.
            pub fn get(self, name: TypeSymbol) -> Option<usize> {
                self.0
                    .iter()
                    .find(|(declared, _)| *declared == name)
                    .map(|(_, index)| *index)
            }
        }
    )*};
}

group_map!(FunctionGroupMap, ShapeGroupMap);

/// How a keyworded call builds the argument record from the shape's slots, in element order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterBinding<'x> {
    /// Slot `i` binds the `i`th name: an `EXPR` head's slot names as written, a binary `OP`'s
    /// `left` and `right`, or a `UNARY OP`'s `operands` at its keyword-first key.
    Named(&'x [BinderSymbol]),
    /// Every slot, in order, packed into one list bound to `operands`: a `UNARY OP` at its binary
    /// key.
    Operands,
}

/// Why a type expression did not elaborate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elaboration {
    /// The type name at `site` is bound to something other than a type.
    NotAType { name: TypeSymbol, site: Site },
    /// A spelling the elaborator does not elaborate, at `site`.
    Unsupported { site: Site },
    /// A projection `Owner.name`, at `site`, naming a member `Owner` does not declare: a union's
    /// tag, or a record's field.
    NoSuchMember {
        owner: Parametric,
        name: Symbol,
        site: Site,
    },
    /// A bound at `site` that names a type variable — a `FOR ALL` name or a signature's head
    /// parameter — or is `Never`.
    Bound { site: Site },
    /// A quantified function type or expression shape at `site`, written anywhere but as the whole
    /// type of a signature's `VAL` member or a signature's keyworded head.
    Quantified { site: Site },
    /// A meet at `site` with an operand naming a `FOR ALL` variable or a signature's head
    /// parameter: each call solves the variable, so the meet cannot be taken where it is written.
    MeetOverVariable { site: Site },
    /// A binder at `site` whose parameter, slot or representation holds a union two of whose
    /// members tie: an argument both admit solves the group either way.
    TiedUnion {
        members: (Parametric, Parametric),
        site: Site,
    },
    /// The load-time reader cannot know the type at `site` before the program runs: the load
    /// pass's cue to leave it for the run, never reported.
    Unknown { site: Site },
}

impl Elaboration {
    /// The site the refusal is about.
    pub fn site(&self) -> Site {
        match self {
            Elaboration::NotAType { site, .. }
            | Elaboration::Unsupported { site }
            | Elaboration::NoSuchMember { site, .. }
            | Elaboration::Bound { site }
            | Elaboration::Quantified { site }
            | Elaboration::MeetOverVariable { site }
            | Elaboration::TiedUnion { site, .. }
            | Elaboration::Unknown { site } => *site,
        }
    }

    /// The refusal as an error value's message, with its names spelled through `symbols` and its
    /// types through `types`.
    pub fn display<'x, 'g, 'run, 'p, 'reg>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x Chain<'g, 'run, 'p, 'reg, '_>,
    ) -> ElaborationDisplay<'x, 'g, 'run, 'p, 'reg> {
        ElaborationDisplay {
            error: self,
            symbols,
            types,
        }
    }
}

/// An [`Elaboration`] beside the interner and registry it renders through.
pub struct ElaborationDisplay<'x, 'g, 'run, 'p, 'reg> {
    error: &'x Elaboration,
    symbols: &'x SymbolInterner,
    types: &'x Chain<'g, 'run, 'p, 'reg, 'x>,
}

impl fmt::Display for ElaborationDisplay<'_, '_, '_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.error {
            Elaboration::NotAType { name, .. } => {
                write!(f, "{} names no type", self.symbols.display(name.symbol()))
            }
            Elaboration::Unsupported { .. } => f.write_str("this type expression is not supported"),
            Elaboration::NoSuchMember { owner, name, .. } => write!(
                f,
                "{} has no member {}",
                display_name(*owner, self.types, self.symbols),
                self.symbols.display(*name)
            ),
            Elaboration::Bound { .. } => f.write_str("a bound names a type variable or Never"),
            Elaboration::Quantified { .. } => {
                f.write_str("a quantified type is written only as a signature's `VAL` member type")
            }
            Elaboration::MeetOverVariable { .. } => {
                f.write_str("a meet's operands name no `FOR ALL` variable or head parameter")
            }
            Elaboration::TiedUnion {
                members: (a, b), ..
            } => write!(
                f,
                "{} and {} tie in one union: neither is more specific, and an argument both admit \
                 solves the group either way",
                display_name(*a, self.types, self.symbols),
                display_name(*b, self.types, self.symbols)
            ),
            Elaboration::Unknown { .. } => f.write_str("this type is known only where it runs"),
        }
    }
}

/// A rigid variable of a load-time type, and where the run reads its value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Variable<'graph> {
    /// The variable's level: the lexical variable the rigid value names it by.
    pub level: usize,
    pub source: Source<'graph>,
}

/// Where the run reads a rigid variable's value.
#[derive(Clone, Copy, Debug)]
pub enum Source<'graph> {
    /// A type name: the coordinate, in the shape the value was typed in, that holds its type.
    Type(Coordinate),
    /// A path variable: the coordinate, in the shape the value was typed in, of the module its
    /// root names — `None` for a root written in place — and the path it stands for.
    Path {
        at: Option<Coordinate>,
        entry: &'graph PathEntry<'graph>,
    },
}

impl PartialEq for Source<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Source::Type(mine), Source::Type(theirs)) => mine == theirs,
            (
                Source::Path { at, entry },
                Source::Path {
                    at: theirs,
                    entry: path,
                },
            ) => at == theirs && std::ptr::eq(*entry, *path),
            _ => false,
        }
    }
}

impl Eq for Source<'_> {}

/// One path of a chain — a type member `label` read through a module-valued expression — typed as
/// a lexical variable of its own, its **path variable**, at a level past every name level of the
/// chain ([design/modules.md § Carriers and paths](../../design/modules.md#carriers-and-paths)).
/// The type channel mints one per distinct root, member chain and label its type expressions
/// spell, and the static pass one more per path it pins a module's type through; the chain's root
/// holds them all. The static pass resolves each one.
#[derive(Clone, Copy)]
pub struct PathEntry<'graph> {
    pub root: Root<'graph>,
    /// The value members read from the root, in order, before `label`.
    pub chain: &'graph [ValueSymbol],
    pub label: TypeSymbol,
    /// The path variable's level.
    pub level: usize,
    /// The application the path is read through: the module `chain` reaches is fitted to it, and
    /// the path is what it fits `label` at. Over the chain's lexical variables; a shape reading the
    /// path locates them in its own [`Statics::throughs`]. `None` where the path reads the module's
    /// own type member.
    pub through: Option<Parametric>,
    /// What the static pass resolved the path to. Written once.
    resolution: &'graph Cell<Option<Resolution<'graph>>>,
}

/// The path entries a pass mints for one chain, found by root, member chain and label: each
/// entry's index in the pass's own run of them, hashed by [`PathEntry::hash_of`].
pub type PathIndex<'p> = BumpHashTable<'p, usize>;

impl<'graph> PathEntry<'graph> {
    /// The hash a [`PathIndex`] finds the entries for `root`, `chain` and `label` by.
    pub fn hash_of(root: Root<'_>, chain: &[ValueSymbol], label: TypeSymbol) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        match root {
            Root::Binder(binder) => {
                (0u8, std::ptr::from_ref(binder.shape).addr(), binder.slot).hash(&mut hasher)
            }
            Root::Inline(site) => (1u8, site).hash(&mut hasher),
        }
        (chain, label).hash(&mut hasher);
        hasher.finish()
    }

    /// Whether this entry is the one for `root`, `chain` and `label`.
    pub fn is(&self, root: Root<'graph>, chain: &[ValueSymbol], label: TypeSymbol) -> bool {
        self.root == root && self.chain == chain && self.label == label
    }

    /// A new entry, unresolved, laid down through `writer`.
    pub fn new(
        writer: Writer<'graph, '_>,
        root: Root<'graph>,
        chain: &[ValueSymbol],
        label: TypeSymbol,
        level: usize,
        through: Option<Parametric>,
    ) -> &'graph PathEntry<'graph> {
        let mut entry = Some(PathEntry {
            root,
            chain: collect(writer, chain.iter().copied()),
            label,
            level,
            through,
            resolution: &writer.fill(1, |_| Cell::new(None))[0],
        });
        &writer.fill(1, |_| entry.take().expect("filled once"))[0]
    }

    /// What the static pass resolved the path to, once it has.
    pub fn resolution(&self) -> Option<Resolution<'graph>> {
        self.resolution.get()
    }

    /// Written once, by the static pass.
    pub fn fix_resolution(&self, resolution: Resolution<'graph>) {
        debug_assert!(self.resolution.get().is_none());
        self.resolution.set(Some(resolution));
    }

    /// The type the path is at every run, where the static pass resolved it to one.
    pub fn closed(&self) -> Option<KType> {
        match self.resolution()?.resolved {
            Resolved::Closed(closed) => Some(closed),
            Resolved::Class(_) => None,
        }
    }
}

impl fmt::Debug for PathEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathEntry")
            .field("level", &self.level)
            .field("chain", &self.chain)
            .field("label", &self.label)
            .finish()
    }
}

/// What the static pass resolved a path to: the content tree of the module it reads its member
/// off, and the type that tree gives it.
#[derive(Clone, Copy)]
pub struct Resolution<'graph> {
    pub tree: Tree<'graph>,
    pub resolved: Resolved,
}

/// The type a path is at load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolved {
    /// The same type at every run.
    Closed(KType),
    /// The path variable at this level: the first path the pass resolved over a tree like this
    /// one's, read at the same label through the same application. Every path of one class is one
    /// type at every run.
    Class(usize),
}

/// A path's root: the binder a name lands at, or an expression written in place, by its site.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Root<'graph> {
    Binder(Binder<'graph>),
    Inline(Site),
}

/// A path spelling one shape's type expressions read: where that shape reads its root, and its
/// entry.
#[derive(Clone, Copy, Debug)]
pub struct PathRead<'graph> {
    pub at: Option<Coordinate>,
    pub entry: &'graph PathEntry<'graph>,
}

/// What the load pass fixed for a type, a callable or a registration: a concrete `C` where it is
/// the same at every run, and a parametric `R` over the run's variables.
#[derive(Clone, Copy, Debug)]
pub enum Static<'graph, C, R = C> {
    /// Not known before the run: elaborate where read.
    Unknown,
    /// The same at every run.
    Closed(C),
    /// Over rigid variables the run supplies, each read at its coordinate.
    Rigid {
        value: R,
        variables: &'graph [Variable<'graph>],
    },
}

/// What the load fixed for a type: concrete where closed, parametric over the run's variables.
pub type StaticType<'graph> = Static<'graph, KType, Parametric>;

/// What the load fixed for an instance's solution, in group order: concrete where closed,
/// parametric over the run's variables where it names a type a run binds. Unlike [`Static`], it
/// has no unknown arm: the load fixes every instance's solution.
#[derive(Clone, Copy, Debug)]
pub enum StaticSolution<'graph> {
    /// The same at every run.
    Closed(&'graph [KType]),
    /// Over rigid variables the run supplies, each read at its coordinate.
    Rigid {
        value: &'graph [Parametric],
        variables: &'graph [Variable<'graph>],
    },
}

/// What the load fixed where a quantified function is read: the solution it instantiated it at,
/// or — a member read at a call's head — that the run reads it as it is, and the call solves its
/// group.
#[derive(Clone, Copy, Debug)]
pub enum InstanceRead<'graph> {
    Solved(StaticSolution<'graph>),
    AsIs,
}

/// What the load fixed for a callable's type.
pub type StaticCallable<'graph> =
    Static<'graph, Callable<'graph, KType>, Callable<'graph, Parametric>>;

/// What the load fixed for a registration's bucket entry.
pub type StaticRegistered<'graph> =
    Static<'graph, Registered<'graph, KType>, Registered<'graph, Parametric>>;

/// What the load fixed about one keyworded use's candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Narrowing<'graph> {
    /// Every candidate on the list, each *maybe*: a call admits them all.
    Full,
    /// The candidates a call selects among, in list order, each beside its verdict — *always* or
    /// *maybe*, never *never*: a *maybe* one is admitted, an *always* one taken as admitted.
    Kept(&'graph [(Candidate, Verdict)]),
    /// The one candidate the load selected: always a [`Candidate::One`], read at this coordinate.
    Selected(Coordinate),
}

/// What the load fixed about one keyworded use: which candidates a call selects among, and what
/// each argument contributes to their solves.
#[derive(Clone, Copy, Debug)]
pub struct UseStatics<'graph> {
    pub narrowing: Narrowing<'graph>,
    /// Per argument, the static type its solving slot is solved from — `Unknown` where the call
    /// reads the carried type. Empty for a use every argument of which does.
    pub contributions: &'graph [StaticType<'graph>],
}

impl UseStatics<'_> {
    /// A use the load fixed nothing about: every candidate kept, every argument read as carried.
    pub const UNFIXED: Self = UseStatics {
        narrowing: Narrowing::Full,
        contributions: &[],
    };

    /// Whether the load fixed nothing about the use.
    pub fn is_unfixed(&self) -> bool {
        matches!(self.narrowing, Narrowing::Full) && self.contributions.is_empty()
    }
}

/// What the load fixed about a frame body's minted types — the program's, a callable's or a quote's
/// code's: whether one can leave it, and how many sites it and the blocks and module bodies it runs
/// mint at run — every minting site but a view over a closed content tree, whose types the load
/// laid in the root ([type tiers](../../design/type-tiers.md#two-classes-of-body)).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyClass {
    /// No porous exit: nothing it mints is reachable once it returns. A callable or a quote's code
    /// that mints at run **caches**: it interns on its write home's scratch, and adopts into its
    /// region what leaves it ([where each body interns](../../design/type-tiers.md#where-each-body-interns)).
    Sealed { mints: u32, caches: bool },
    /// A porous exit: its return, or an argument of a call it makes into a frame, can carry a
    /// minted type out.
    Porous { mints: u32 },
}

/// How the load classed one minting site of a body: by what its content reads
/// ([where each body interns](../../design/type-tiers.md#where-each-body-interns)).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mint {
    /// Over a closed content tree: the same content at every run, its types in the root.
    Closed,
    /// The same content at every frame of one binding: interned into the warm tier, the
    /// specialization the frame borrows, or where it was born with none, the scratch branch its
    /// asker's park holds.
    Invariant,
    /// Content that may differ per hop: interned into the cold tier.
    Varying,
}

/// What the load fixed about where a body's types intern at run: whether its frame caches, how
/// each of its own minting sites is classed, and which of its composite sites — list, dict and
/// record literals and constructions — build values whose own types are warm.
#[derive(Clone, Copy, Debug)]
pub struct Caching<'graph> {
    /// Whether the frame body this shape runs in caches: its own class for a callable or a quote's
    /// code, its frame's for a block or a module body.
    pub caches: bool,
    /// Each of the shape's own minting sites, sorted by site, beside its class.
    pub mints: Keyed<'graph, (Site, Mint)>,
    /// Each composite site whose parts the load classed invariant, sorted.
    pub warm: Keyed<'graph, Site>,
    /// On a caching frame body's own record, when it or a block or module body it runs has an
    /// invariant minting site or an invariant composite site: the frame's parameter slots those
    /// sites read, sorted, which may be none. Each is a parameter whose arguments digest at a
    /// constant cost: one declared under `Module` or as a type value, or a type binder. A call to
    /// the frame seeks the specialization these slots' arguments key. `None` on every other
    /// record.
    pub invariant: Option<&'graph [Slot]>,
}

/// The value channel's load-time facts for one body: a static type for every value expression and
/// statement — under which every type the run carries there lies — and each keyworded use's
/// narrowing and contributions.
#[derive(Clone, Copy, Debug)]
pub struct Statics<'graph> {
    /// Each part the evaluator reads as a value, by site, sorted by site, beside its static type:
    /// an interval.
    pub parts: Keyed<'graph, (Site, Interval<Parametric>)>,
    /// Each statement's static type, an interval, by index into the shape's body; at most `Any`
    /// for one that binds nothing typed.
    pub statements: &'graph [Interval<Parametric>],
    /// Each slot's static type, an interval, by index: a value binder's, a registration's function
    /// type, and for a type name the type of the type value it holds. A quantified callable's binder
    /// is its scheme. Read by tests alone.
    #[cfg(test)]
    pub binders: &'graph [DeclaredType<Interval<Parametric>>],
    /// Each `:!` whose operand's static upper end lies under its type, and each annotated binder's
    /// type part whose value's does, sorted by site: the run checks nothing there.
    pub settled: Keyed<'graph, Site>,
    /// Each site where the load read a quantified function, sorted by site, beside what it fixed
    /// there: the solution it instantiated it at, closed or over the run's variables, or that the
    /// run reads it as it is.
    pub instances: Keyed<'graph, (Site, InstanceRead<'graph>)>,
    /// Each keyworded use the load fixed something about, by its node's site, sorted by site: its
    /// narrowing and its contributions. A use absent here is [`UseStatics::UNFIXED`].
    pub uses: Keyed<'graph, (Site, UseStatics<'graph>)>,
    /// Each call by name's contributions, by its argument part's site, sorted by site: per
    /// parameter of its callee in symbol order, the static type that parameter is solved from —
    /// `Unknown` where the frame reads the carried type.
    pub named: Keyed<'graph, (Site, &'graph [StaticType<'graph>])>,
    /// A frame body's class; `None` for a block or a module body, which count toward the frame
    /// body enclosing them.
    pub class: Option<BodyClass>,
    /// This shape's own minting sites, sorted: each `:|` ascription, and each component of nominal
    /// declarations the load leaves unknown, by its first member's declaration node.
    pub minting: Keyed<'graph, Site>,
    /// Each `:|` ascription's site at a closed signature, sorted, beside its view's content tree.
    pub trees: Keyed<'graph, (Site, &'graph Tree<'graph>)>,
    /// Each path this shape's type expressions read whose [`through`](PathEntry::through) names a
    /// lexical variable, by the path's level, beside where this shape reads each of them.
    pub throughs: Keyed<'graph, (usize, &'graph [Variable<'graph>])>,
}

/// How the run binds a path variable: the type a path's entry reads through the module at a
/// coordinate of the view a value is solved against. The elaborator's `bound_path` is the one
/// reader, passed in from above.
pub type Paths<'f, 'graph> = &'f dyn Fn(Coordinate, &'graph PathEntry<'graph>) -> Option<KType>;

/// What each of `variables` reads through `view`, as the bindings a substitution takes: an entry
/// per level up to the largest, each variable's the handle of the type its coordinate holds — or,
/// for a path variable, the type `paths` reads through its root — and `Never` where no variable is
/// listed: a gap no free variable of the rigid value names. A path the load resolved closed is that
/// type, with no read. `None` when a coordinate holds anything but a type, or a path reads none.
pub fn solutions<'graph, 'x, XF: KnottedFamily<'graph>>(
    variables: &[Variable<'graph>],
    view: &ActivationView<'graph, '_, XF>,
    scratch: BumpAllocator<'x>,
    paths: Paths<'_, 'graph>,
) -> Option<BumpVec<'x, KType>> {
    let len = variables.iter().map(|variable| variable.level + 1).max();
    let mut bindings = BumpVec::with_capacity_in(len.unwrap_or(0), scratch);
    bindings.resize(len.unwrap_or(0), KType::NEVER);
    for variable in variables {
        bindings[variable.level] = match variable.source {
            Source::Type(at) => match view.read(at) {
                Value::Type(value) => value.handle(),
                _ => return None,
            },
            Source::Path { at, entry } => match entry.closed() {
                Some(closed) => closed,
                None => paths(at?, entry)?,
            },
        };
    }
    Some(bindings)
}

impl<'graph, C, R> Static<'graph, C, R> {
    /// Whether the value is rigid over a path variable.
    pub fn over_paths(&self) -> bool {
        match self {
            Static::Rigid { variables, .. } => variables
                .iter()
                .any(|variable| matches!(variable.source, Source::Path { .. })),
            _ => false,
        }
    }

    /// The value where it runs, read through `view`: a closed value as it is, and a rigid one with
    /// its variables replaced by what their coordinates read, through `substitute`. `None` for an
    /// unknown value, where a variable's coordinate holds no type, and where `substitute` finds no
    /// concrete value.
    pub fn solved<'x, XF: KnottedFamily<'graph>>(
        self,
        view: &ActivationView<'graph, '_, XF>,
        scratch: BumpAllocator<'x>,
        paths: Paths<'_, 'graph>,
        substitute: impl FnOnce(R, &[KType]) -> Option<C>,
    ) -> Option<C> {
        match self {
            Static::Unknown => None,
            Static::Closed(value) => Some(value),
            Static::Rigid { value, variables } => {
                let bindings = solutions(variables, view, scratch, paths)?;
                substitute(value, &bindings)
            }
        }
    }
}
