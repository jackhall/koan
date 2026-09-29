//! The vocabulary of load-time types: what [the elaborator's load pass](../elaborate/README.md#the-type-channel-at-load)
//! fixes for a shape before the program runs, and the records it fixes.
//!
//! `scope` sits below `elaborate`, so the records the elaborator hands back for a callable — its
//! [`Callable`] type, the [`Registered`] shape its bucket holds, where each `FOR ALL` name landed
//! ([`Canonical`]) — and its refusal ([`Elaboration`]) live here, beside the shape whose write-once
//! cells hold them. A cell holds a [`Static`]: unknown, closed, or rigid over [`Variable`]s the run
//! supplies, which [`solutions`] reads through an activation for one substitution.
//!
//! The value channel's records live here too: the [`Statics`] the language's load pass fixes for a
//! body, and the [`Narrowing`] of each keyworded use's candidates.
//!
//! See [README.md § Load-time types](README.md#load-time-types).

use std::fmt;

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{KType, TypeRegistry, display_name};
use crate::values::{KnottedFamily, Value};

use super::activation::ActivationView;
use super::shape::{Candidate, Coordinate, Site};

/// Where a `FOR ALL` name the declaration wrote landed in a canonical group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Canonical {
    /// The group's variable at this index: a call binds the name to what the group solves it to.
    At(usize),
    /// Dropped by canonical form: a call binds the name to its bound.
    Dropped { bound: KType },
}

/// A callable's type, and how its `FOR ALL` group's declaration order maps onto that type's
/// canonical group — what a call needs to bind each type parameter to its solution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Callable<'x> {
    pub ktype: KType,
    /// Each `FOR ALL` name the declaration wrote, in written order, with where it landed in
    /// `ktype`'s canonical group. Empty for an unquantified callable.
    ///
    /// The **name** is the key, not the position: a callee's type-parameter slots reach its frame
    /// symbol-sorted, not in written order, and `ktype`'s own `quantifiers` cannot stand in for
    /// this because alpha-variants intern to one node and it holds whichever spelling interned
    /// first.
    pub quantifier_map: &'x [(TypeSymbol, Canonical)],
    /// What the registration the callable is born for puts in its bucket; `None` for a callable no
    /// registration binds — a `FN`, or a combined statement's name.
    pub registered: Option<Registered<'x>>,
}

/// What a registration's bucket holds of the function it binds, and what a keyworded call of it
/// reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registered<'x> {
    /// The expression shape: the function type's parameters laid over the registration's key,
    /// ranked by the registration's classes.
    pub shape: KType,
    /// Each `FOR ALL` name the declaration wrote, in written order, with where it landed in
    /// `shape`'s canonical group — which numbers the variables by first occurrence in element
    /// order, not as `ktype`'s does.
    pub quantifier_map: &'x [(TypeSymbol, Canonical)],
    pub parameters: ParameterBinding<'x>,
}

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
        owner: KType,
        name: Symbol,
        site: Site,
    },
    /// A bound at `site` that names a type variable — a `FOR ALL` name or a signature's abstract
    /// member — or is `Never`.
    Bound { site: Site },
    /// A meet at `site` of two signatures that rank one keyword pattern two ways.
    RankingDisagrees { site: Site },
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
            | Elaboration::RankingDisagrees { site }
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
            Elaboration::RankingDisagrees { .. } => {
                f.write_str("a meet of two signatures ranks one key two ways")
            }
            Elaboration::Unknown { .. } => f.write_str("this type is known only where it runs"),
        }
    }
}

/// A rigid variable of a load-time type, and where the run reads its value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Variable {
    /// The variable's index: the `Quantified` index the rigid value names it by.
    pub index: usize,
    /// The coordinate, in the shape the value was typed in, that holds the variable's type.
    pub at: Coordinate,
}

/// What the load pass fixed for a type, a callable or a registration.
#[derive(Clone, Copy, Debug)]
pub enum Static<'graph, T> {
    /// Not known before the run: elaborate where read.
    Unknown,
    /// The same at every run.
    Closed(T),
    /// Over rigid variables the run supplies, each read at its coordinate.
    Rigid {
        value: T,
        variables: &'graph [Variable],
    },
}

/// What the load fixed about one keyworded use's candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Narrowing<'graph> {
    /// Every candidate stays, and a call admits them all.
    Full,
    /// The candidates that can admit, in list order: every other can never admit what the run
    /// passes.
    Kept(&'graph [Candidate]),
    /// The one candidate the load selected: always a [`Candidate::One`], read at this coordinate.
    Selected(Coordinate),
}

/// The value channel's load-time facts for one body: a static type for every value expression and
/// value binder — under which every type the run carries there lies — and each keyworded use's
/// narrowing.
#[derive(Clone, Copy, Debug)]
pub struct Statics<'graph> {
    /// Each part the evaluator reads as a value, by site, sorted by site.
    pub parts: &'graph [(Site, KType)],
    /// Each statement, by index into the shape's body; `Any` for one that binds nothing typed.
    pub statements: &'graph [KType],
    /// Each slot, by index: a value binder's static type, a registration's function type, and for a
    /// type name the type of the type value it holds.
    pub binders: &'graph [KType],
    /// Each keyworded use's narrowing, parallel to the shape's candidate lists.
    pub narrowings: &'graph [Narrowing<'graph>],
}

/// What each of `variables` reads through `view`, as the bindings a substitution takes: an entry
/// per index up to the largest, each variable's the handle of the type its coordinate holds, and
/// `Never` where no variable is listed — a gap no free variable of the rigid value names. `None`
/// when a coordinate holds anything but a type.
pub fn solutions<'graph, 'x, XF: KnottedFamily<'graph>>(
    variables: &[Variable],
    view: &ActivationView<'graph, '_, XF>,
    scratch: BumpAllocator<'x>,
) -> Option<BumpVec<'x, KType>> {
    let len = variables.iter().map(|variable| variable.index + 1).max();
    let mut bindings = BumpVec::with_capacity_in(len.unwrap_or(0), scratch);
    bindings.resize(len.unwrap_or(0), KType::NEVER);
    for variable in variables {
        match view.read(variable.at) {
            Value::Type(value) => bindings[variable.index] = value.handle(),
            _ => return None,
        }
    }
    Some(bindings)
}

impl<'graph, T> Static<'graph, T> {
    /// The value where it runs, read through `view`: a closed value as it is, and a rigid one with
    /// its variables replaced by what their coordinates read, through `substitute`. `None` for an
    /// unknown value, and where a variable's coordinate holds no type.
    pub fn solved<'x, XF: KnottedFamily<'graph>>(
        self,
        view: &ActivationView<'graph, '_, XF>,
        scratch: BumpAllocator<'x>,
        substitute: impl FnOnce(T, &[KType]) -> T,
    ) -> Option<T> {
        match self {
            Static::Unknown => None,
            Static::Closed(value) => Some(value),
            Static::Rigid { value, variables } => {
                let bindings = solutions(variables, view, scratch)?;
                Some(substitute(value, &bindings))
            }
        }
    }
}
