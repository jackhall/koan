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
//! body, and the [`Narrowing`] of each keyworded use's candidates.
//!
//! See [README.md § Load-time types](README.md#load-time-types).

use std::fmt;

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, TypeRegistry, Verdict, display_name,
};
use crate::values::{KnottedFamily, Value};

use super::activation::ActivationView;
use super::shape::{Candidate, Coordinate, Site};

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
            | Elaboration::Unknown { site } => *site,
        }
    }

    /// The refusal as an error value's message, with its names spelled through `symbols` and its
    /// types through `types`.
    pub fn display<'x, 'run>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x TypeRegistry<'run>,
    ) -> ElaborationDisplay<'x, 'run> {
        ElaborationDisplay {
            error: self,
            symbols,
            types,
        }
    }
}

/// An [`Elaboration`] beside the interner and registry it renders through.
pub struct ElaborationDisplay<'x, 'run> {
    error: &'x Elaboration,
    symbols: &'x SymbolInterner,
    types: &'x TypeRegistry<'run>,
}

impl fmt::Display for ElaborationDisplay<'_, '_> {
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
            Elaboration::Unknown { .. } => f.write_str("this type is known only where it runs"),
        }
    }
}

/// A rigid variable of a load-time type, and where the run reads its value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Variable {
    /// The variable's level: the lexical variable the rigid value names it by.
    pub level: usize,
    /// The coordinate, in the shape the value was typed in, that holds the variable's type.
    pub at: Coordinate,
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

/// The value channel's load-time facts for one body: a static type for every value expression and
/// value binder — under which every type the run carries there lies — and each keyworded use's
/// narrowing.
#[derive(Clone, Copy, Debug)]
pub struct Statics<'graph> {
    /// Each part the evaluator reads as a value, by site, sorted by site, beside its static type:
    /// an interval.
    pub parts: &'graph [(Site, Interval<Parametric>)],
    /// Each statement's static type, an interval, by index into the shape's body; at most `Any`
    /// for one that binds nothing typed.
    pub statements: &'graph [Interval<Parametric>],
    /// Each slot's static type, an interval, by index: a value binder's, a registration's function
    /// type, and for a type name the type of the type value it holds. A quantified callable's binder
    /// is its scheme.
    pub binders: &'graph [DeclaredType<Interval<Parametric>>],
    /// Each keyworded use's narrowing, parallel to the shape's candidate lists.
    pub narrowings: &'graph [Narrowing<'graph>],
    /// Each `:!` whose operand's static upper end lies under its type, and each annotated binder's
    /// type part whose value's does, sorted by site: the run checks nothing there.
    pub settled: &'graph [Site],
    /// Each name read at an instance site, by site, sorted by site, beside the solution the load
    /// instantiated its quantified function at.
    pub instances: &'graph [(Site, &'graph [KType])],
}

/// What each of `variables` reads through `view`, as the bindings a substitution takes: an entry
/// per level up to the largest, each variable's the handle of the type its coordinate holds, and
/// `Never` where no variable is listed — a gap no free variable of the rigid value names. `None`
/// when a coordinate holds anything but a type.
pub fn solutions<'graph, 'x, XF: KnottedFamily<'graph>>(
    variables: &[Variable],
    view: &ActivationView<'graph, '_, XF>,
    scratch: BumpAllocator<'x>,
) -> Option<BumpVec<'x, KType>> {
    let len = variables.iter().map(|variable| variable.level + 1).max();
    let mut bindings = BumpVec::with_capacity_in(len.unwrap_or(0), scratch);
    bindings.resize(len.unwrap_or(0), KType::NEVER);
    for variable in variables {
        match view.read(variable.at) {
            Value::Type(value) => bindings[variable.level] = value.handle(),
            _ => return None,
        }
    }
    Some(bindings)
}

impl<'graph, C, R> Static<'graph, C, R> {
    /// The value where it runs, read through `view`: a closed value as it is, and a rigid one with
    /// its variables replaced by what their coordinates read, through `substitute`. `None` for an
    /// unknown value, where a variable's coordinate holds no type, and where `substitute` finds no
    /// concrete value.
    pub fn solved<'x, XF: KnottedFamily<'graph>>(
        self,
        view: &ActivationView<'graph, '_, XF>,
        scratch: BumpAllocator<'x>,
        substitute: impl FnOnce(R, &[KType]) -> Option<C>,
    ) -> Option<C> {
        match self {
            Static::Unknown => None,
            Static::Closed(value) => Some(value),
            Static::Rigid { value, variables } => {
                let bindings = solutions(variables, view, scratch)?;
                substitute(value, &bindings)
            }
        }
    }
}
