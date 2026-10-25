//! The vocabulary of load-time types: what [the elaborator's load pass](../elaborate/README.md#the-type-channel-at-load)
//! fixes for a shape before the program runs, and the records it fixes.
//!
//! `scope` sits below `elaborate`, so the records the elaborator hands back for a callable — its
//! [`Callable`] type, the [`Registered`] shape its bucket holds, where each `FOR ALL` name landed
//! in its group — and its refusal ([`Elaboration`]) live here, beside the shape whose facts the
//! load's records hold. A fact is a [`Static`]: unknown, closed — a concrete value, the same at
//! every run — or rigid over [`Variable`]s — a parametric value over the lexical variables the run
//! supplies, by level — which [`solutions`] reads through an activation for one substitution.
//!
//! The value channel's records live here too: the [`UseStatics`] the load's walk fixes for each
//! keyworded use: its candidates' [`Narrowing`] and its
//! arguments' contributions, and a frame body's [`BodyClass`].
//!
//! See [README.md § Load-time types](README.md#load-time-types).

use std::fmt;
use std::hash::{Hash, Hasher};

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, TypeNode, Verdict, display_name, outranks,
};
use crate::values::Chain;

use super::keyed::Keyed;
use super::shape::{Candidate, Coordinate, Segment, Site, Slot};
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
    /// A type at `site` reads the name `name`, which no layer the run reads binds: what a rigid
    /// fact reaching the run refuses as, worded as the load words a name with no binding.
    Unbound { name: Symbol, site: Site },
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
            | Elaboration::Unknown { site }
            | Elaboration::Unbound { site, .. } => *site,
        }
    }

    /// The refusal as an error value's message, with its names spelled through `symbols` and its
    /// types through `types`.
    pub fn display<'x, 'run, 'p, 'reg>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x Chain<'run, 'p, 'reg, '_>,
    ) -> ElaborationDisplay<'x, 'run, 'p, 'reg> {
        ElaborationDisplay {
            error: self,
            symbols,
            types,
        }
    }
}

/// An [`Elaboration`] beside the interner and registry it renders through.
pub struct ElaborationDisplay<'x, 'run, 'p, 'reg> {
    error: &'x Elaboration,
    symbols: &'x SymbolInterner,
    types: &'x Chain<'run, 'p, 'reg, 'x>,
}

impl fmt::Display for ElaborationDisplay<'_, '_, '_, '_> {
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
            Elaboration::Unbound { name, .. } => write!(
                f,
                "`{}` names no binding visible here",
                self.symbols.display(*name)
            ),
        }
    }
}

/// A rigid variable of a load-time type, and where the run reads its value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Variable {
    /// The variable's level: the lexical variable the rigid value names it by.
    pub level: usize,
    pub source: Source,
}

/// Where the run reads a rigid variable's value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// A type name: the coordinate, in the shape the value was typed in, that holds its type.
    Type(Coordinate),
    /// A path variable: the coordinate, in the shape the value was typed in, of the module its
    /// root names — `None` for a root written in place — and the path it stands for, by the
    /// segment of the body rooting its chain and its level there.
    Path {
        at: Option<Coordinate>,
        root: Segment,
        level: usize,
    },
}

/// One path of a chain — a type member `label` read through a module-valued expression — typed as
/// a lexical variable of its own, its **path variable**, at a level past every name level of the
/// chain ([design/modules.md § Carriers and paths](../../design/modules.md#carriers-and-paths)).
/// The shape numbers one per distinct root, member chain and label its type expressions spell,
/// and the load's walk one more per path it pins a module's type through, past them. The walk
/// resolves each one in the unit that reads it first ([`LoadFacts::resolution`](super::LoadFacts::resolution)).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PathEntry<'graph> {
    pub root: Root,
    /// The value members read from the root, in order, before `label`.
    pub chain: &'graph [ValueSymbol],
    pub label: TypeSymbol,
    /// The path variable's level.
    pub level: usize,
    /// The application the path is read through: the module `chain` reaches is fitted to it, and
    /// the path is what it fits `label` at. Over the chain's lexical variables, which a layer
    /// binding them closes. `None` where the path reads the module's own type member.
    pub through: Option<Parametric>,
}

impl<'graph> PathEntry<'graph> {
    /// The hash the walk finds the entries for `root`, `chain` and `label` by.
    pub fn hash_of(root: Root, chain: &[ValueSymbol], label: TypeSymbol) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        match root {
            Root::Binder(binder) => (0u8, binder).hash(&mut hasher),
            Root::Inline { site, .. } => (1u8, site).hash(&mut hasher),
        }
        (chain, label).hash(&mut hasher);
        hasher.finish()
    }

    /// Whether this entry is the one for `root`, `chain` and `label`.
    pub fn is(&self, root: Root, chain: &[ValueSymbol], label: TypeSymbol) -> bool {
        self.root == root && self.chain == chain && self.label == label
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

/// What the load's walk resolved a path to: the content tree of the module it reads its member
/// off, and the type that tree gives it.
#[derive(Clone, Copy)]
pub struct Resolution<'graph> {
    pub tree: Tree<'graph>,
    pub resolved: Resolved,
    /// The signature the root holds at every run, where the path reads no type member off it:
    /// every run refuses the path.
    pub lacks: Option<KType>,
}

impl Resolution<'_> {
    /// The type the path is at every run, where the load resolved it to one.
    pub fn closed(&self) -> Option<KType> {
        match self.resolved {
            Resolved::Closed(closed) => Some(closed),
            Resolved::Class(_) => None,
        }
    }
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

/// A path's root: the binder a name lands at, or an expression written in place, by its site and
/// the statement of its shape's body it is written in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Root {
    Binder(Binder),
    Inline { site: Site, statement: u32 },
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
        variables: &'graph [Variable],
    },
}

/// What the load fixed for a type: concrete where closed, parametric over the run's variables.
pub type StaticType<'graph> = Static<'graph, KType, Parametric>;

/// What one variable of a selected candidate's group solves to at every run of its call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Point {
    /// The same type at every run.
    Closed(KType),
    /// A type over the run's lexical variables, read through its [`Points`]' variables: the load's
    /// record alone, since the layer binding them closes it before any run reads it.
    Rigid(Parametric),
    /// The type the argument at this slot carries: the variable is the slot's whole type, its own
    /// class's, which no other slot of the class names, and the argument contributes nothing.
    Carried(u16),
}

/// The points a selected candidate's group solves to, in group order, beside where the run reads
/// the lexical variables a rigid one names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Points<'graph> {
    pub points: &'graph [Point],
    pub variables: &'graph [Variable],
}

impl<'graph> Points<'graph> {
    /// The points of a group of no variables.
    pub const NONE: Points<'static> = Points {
        points: &[],
        variables: &[],
    };

    /// The variable at `at`'s closed point, where it has one.
    pub fn closed(&self, at: usize) -> Option<KType> {
        match self.points.get(at)? {
            Point::Closed(point) => Some(*point),
            Point::Rigid(_) | Point::Carried(_) => None,
        }
    }

    /// The solution where the call runs over operands carrying `arguments`: a closed point as it
    /// is, and a carried one its argument's type. `None` where an argument is missing, or a point
    /// is rigid, which no run reads.
    pub fn solved<'x>(
        &self,
        scratch: BumpAllocator<'x>,
        arguments: &[KType],
    ) -> Option<BumpVec<'x, KType>> {
        let mut solved = BumpVec::with_capacity_in(self.points.len(), scratch);
        for point in self.points {
            solved.push(match point {
                Point::Closed(point) => *point,
                Point::Rigid(_) => return None,
                Point::Carried(slot) => *arguments.get(usize::from(*slot))?,
            });
        }
        Some(solved)
    }
}

/// What a view over a module whose content the load sees all of carries for one keyworded member
/// of its signature: the module's registrations covering it, ranked first among themselves, by
/// their positions in its registration run — so two overloads of one shape stay two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pick<'graph> {
    /// The keyworded member, by its position among the signature's.
    pub member: u16,
    pub offered: &'graph [u32],
}

/// What the load fixed for an instance: the solution it is made at, in group order, and the
/// instance's type — concrete where closed, parametric over the run's variables where the solution
/// names a type a run binds. Unlike [`Static`], it has no unknown arm: the load fixes every
/// instance it records.
#[derive(Clone, Copy, Debug)]
pub enum StaticInstance<'graph> {
    /// The same at every run.
    Closed {
        solution: &'graph [KType],
        ktype: KType,
    },
    /// Over rigid variables the run supplies, each read at its coordinate: one list for both.
    Rigid {
        solution: &'graph [Parametric],
        ktype: Parametric,
        variables: &'graph [Variable],
    },
}

/// What the load fixed where a quantified function is read: the solution it instantiated it at,
/// or — a member read at a call's head — that the run reads it as it is, and the call solves its
/// group.
#[derive(Clone, Copy, Debug)]
pub enum InstanceRead<'graph> {
    Solved(StaticInstance<'graph>),
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
    /// The candidates a call selects among, and how they rank.
    Kept(&'graph Kept<'graph>),
    /// The one candidate the load selected: always a [`Candidate::One`], read at this coordinate.
    Selected(Coordinate),
}

/// The candidates a call selects among, in list order, each beside its verdict — *always* or
/// *maybe*, never *never*: a *maybe* one is admitted, an *always* one taken as admitted — and how
/// those that admit rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kept<'graph> {
    pub entries: &'graph [(Candidate, Verdict)],
    /// Beside each entry, the points an *always* one's group solves to: [`Points::NONE`] for a
    /// *maybe* one and one of no group.
    pub points: &'graph [Points<'graph>],
    pub elimination: Elimination<'graph>,
}

/// How a kept list's candidates rank: read off a table the load recorded, or ranked at the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elimination<'graph> {
    /// Ranked at the call: a spread, a shape the load cannot read, a rigid one, or a list of more
    /// than [`Table::MOST`] candidates.
    Open,
    Table(Table<'graph>),
}

/// The elimination table of a kept list of closed shapes: at each class, which of the list's
/// candidates each one strictly outranks there, beside which are builtins. No single rank order
/// exists — three candidates may beat each other in a cycle pair by pair — so each class keeps
/// every pair's answer, by position in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Table<'graph> {
    /// `beats[class * len + at]`: the positions the candidate at `at` strictly outranks at `class`.
    beats: &'graph [u64],
    builtins: u64,
    len: u16,
}

impl<'graph> Table<'graph> {
    /// The longest list a table ranks: one bit per candidate.
    pub const MOST: usize = 64;

    /// The table of `len` candidates over `classes` classes, `outranks(class, a, b)` answering
    /// whether the candidate at `a` strictly outranks the one at `b` there, laid through `lay`;
    /// `builtin(at)` says which are builtins. `None` past [`MOST`](Self::MOST).
    pub fn laid(
        lay: impl Lays<'graph>,
        len: usize,
        classes: usize,
        mut outranks: impl FnMut(u16, usize, usize) -> bool,
        builtin: impl Fn(usize) -> bool,
    ) -> Option<Self> {
        if len > Self::MOST {
            return None;
        }
        let beats = lay.fill(classes * len, |at| {
            let class = u16::try_from(at / len).expect("a shape has fewer than 65536 slots");
            let a = at % len;
            ((0..len).filter(|b| *b != a && outranks(class, a, *b)))
                .fold(0, |mask, b| mask | 1 << b)
        });
        let builtins = (0..len)
            .filter(|at| builtin(*at))
            .fold(0u64, |mask, at| mask | 1 << at);
        Some(Table {
            beats,
            builtins,
            len: len as u16,
        })
    }

    /// The table of `shapes`, registered shapes under one key and one ranking, each pair's answer
    /// read from the lattice's [`outranks`] through `types`, laid through `lay`; `builtin` says
    /// which are builtins. `ranked` is told of each pair ranked. `None` past [`MOST`](Self::MOST).
    pub fn ranked(
        lay: impl Lays<'graph>,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        shapes: &[DeclaredType<Parametric>],
        builtin: impl Fn(usize) -> bool,
        ranked: &mut dyn FnMut(),
    ) -> Option<Self> {
        // Only the ranking is read, which a scheme's body spells as a type's does.
        let classes = shapes
            .first()
            .map_or(0, |shape| match shape.view(types).body {
                TypeNode::ExpressionShape { classes, .. } => classes
                    .iter()
                    .max()
                    .map_or(0, |highest| usize::from(highest) + 1),
                _ => unreachable!("a registered shape is an expression shape"),
            });
        let mut outranks = |class, a: usize, b: usize| {
            ranked();
            outranks(types, scratch, shapes[a], shapes[b], class)
        };
        Self::laid(lay, shapes.len(), classes, &mut outranks, builtin)
    }

    /// Which of those `admitted` names, by position, survive the class-by-class elimination: at
    /// each class, a survivor another survivor strictly outranks there drops out.
    pub fn survivors(&self, admitted: u64) -> u64 {
        let len = usize::from(self.len);
        let mut survivors = admitted;
        for class in self.beats.chunks(len.max(1)) {
            if survivors.count_ones() <= 1 {
                break;
            }
            let beaten = (0..len)
                .filter(|at| survivors & 1 << at != 0)
                .fold(0u64, |mask, at| mask | class[at]);
            survivors &= !beaten;
        }
        survivors
    }

    /// The candidate a call runs among those `admitted` names, by position: the lone
    /// [survivor](Self::survivors), else the first builtin among several in list order. `Err` with
    /// the survivors' count where several survive and none is a builtin, or where none admitted.
    pub fn decide(&self, admitted: u64) -> Result<usize, usize> {
        let survivors = self.survivors(admitted);
        match survivors.count_ones() {
            1 => Ok(survivors.trailing_zeros() as usize),
            count => match survivors & self.builtins {
                0 => Err(count as usize),
                builtins => Ok(builtins.trailing_zeros() as usize),
            },
        }
    }

    /// This table laid again through `lay`.
    pub fn relaid<'to>(&self, lay: impl Lays<'to>) -> Table<'to> {
        Table {
            beats: collect(lay, self.beats.iter().copied()),
            builtins: self.builtins,
            len: self.len,
        }
    }
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
    /// The same content at every frame of one binding: interned into the warm tier — the agent
    /// tier, where the frame runs under a settlement — and into the cold tier where it runs
    /// under none.
    Invariant,
    /// Content that may differ per hop: interned into the cold tier.
    Varying,
}

/// What the load fixed about where a body's types intern at run: whether its frame caches, how
/// each of its own minting sites is classed, and which of its composite sites — list, dict and
/// record literals and constructions — build values whose own types are warm. On a callable's own
/// record it also says which of its parameters may key its settlement.
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
    /// the frame seeks the settlement these slots' arguments key. `None` on every other
    /// record.
    pub invariant: Option<&'graph [Slot]>,
}

/// At a `:|` over a module binder read bare: the module's body, and per capture its content
/// composes that the load read from a slot, the capture's index beside the slot's coordinate from
/// the `:|`'s own activation, where that slot's digest memo sits.
#[derive(Clone, Copy, Debug)]
pub struct ViewSlots<'graph> {
    pub body: Segment,
    pub captures: &'graph [(u32, Coordinate)],
}

/// A call argument that is a bare read of a digested slot at `from`, whose memo the call hands to
/// the callee's parameter it binds.
#[derive(Clone, Copy, Debug)]
pub struct Handoff {
    pub to: HandedTo,
    pub from: Coordinate,
}

/// Which argument a [`Handoff`] is: a call by name's record field, or a keyworded use's slot by
/// its position among the use's slots, keywords aside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandedTo {
    Field(Symbol),
    Position(u32),
}

impl<C, R> Static<'_, C, R> {
    /// Whether the value is rigid over a path variable.
    pub fn over_paths(&self) -> bool {
        match self {
            Static::Rigid { variables, .. } => variables
                .iter()
                .any(|variable| matches!(variable.source, Source::Path { .. })),
            _ => false,
        }
    }
}
