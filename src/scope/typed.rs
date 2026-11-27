//! The vocabulary of load-time types: what [the elaborator's load pass](../elaborate/README.md#the-type-channel-at-load)
//! fixes for a shape before the program runs, and the records it fixes.
//!
//! `scope` sits below `elaborate`, so the records the elaborator hands back for a callable — its
//! [`Callable`] type, the [`Registered`] shape its bucket holds, where each `FOR ALL` name landed
//! in its group — and its refusal ([`Elaboration`]) live here, beside the shape whose facts the
//! load's records hold. A fact is a [`Static`]: unknown, closed — a concrete value, the same at
//! every run — or rigid over [`Variable`]s — a parametric value over the lexical variables the run
//! supplies, a name's by level and a path class's by class — which [`solutions`] reads through an
//! activation for one substitution.
//!
//! The value channel's records live here too: the [`UseStatics`] the load's walk fixes for each
//! keyworded use: its candidates' [`Narrowing`] and its
//! arguments' contributions, and a frame body's [`BodyClass`].
//!
//! See [README.md § Load-time types](README.md#load-time-types).

use std::fmt;

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, PathClass, Scheme, TypeNode, Verdict, display_name,
    outranks,
};
use crate::values::Chain;
use crate::values::digest::{DigestHasher, Tag};

use super::shape::{Candidate, Coordinate, Segment, Site};
use super::site::Selection;
use super::tree::{Binder, Identity, Tree};

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
    /// The bounds `ktype`'s group writes over an encloser's names, by position in that group:
    /// `ktype` holds each at its upper end. Empty for a callable the load closed.
    pub rebounds: Rebounds<'x>,
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
    /// The bounds `shape`'s group writes over an encloser's names, by position in that group.
    pub rebounds: Rebounds<'x>,
}

/// The bounds a scheme's `FOR ALL` group writes over an encloser's names, each beside its
/// variable's position in that scheme's group. The scheme holds each such bound at its upper end
/// at every binding, and a binding closing every one rebuilds it ([`Shaped::rebuilt`]). Empty
/// where every bound is closed. A position names one scheme's numbering: a function type and its
/// registered shape each carry their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rebounds<'x>(pub &'x [(u8, Parametric)]);

impl<'x> Rebounds<'x> {
    pub const NONE: Rebounds<'static> = Rebounds(&[]);

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// These bounds laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Rebounds<'to> {
        match self.0 {
            [] => Rebounds(&[]),
            bounds => Rebounds(collect(lay, bounds.iter().copied())),
        }
    }
}

/// A value's static type where a binder or a call's head holds it: a type's interval, or a
/// quantified callable's scheme beside the bounds its group writes over names ([`Rebounds`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holding<'x> {
    Type(Interval),
    Scheme(Scheme, Rebounds<'x>),
}

impl From<Interval> for Holding<'_> {
    fn from(typed: Interval) -> Self {
        Holding::Type(typed)
    }
}

impl From<DeclaredType<Interval>> for Holding<'_> {
    /// A type's interval, or a scheme whose every bound is closed.
    fn from(declared: DeclaredType<Interval>) -> Self {
        match declared {
            DeclaredType::Type(typed) => Holding::Type(typed),
            DeclaredType::Scheme(scheme) => Holding::scheme(scheme),
        }
    }
}

impl<'x> Holding<'x> {
    /// A scheme whose every bound is closed.
    pub fn scheme(scheme: Scheme) -> Self {
        Holding::Scheme(scheme, Rebounds(&[]))
    }

    /// The interval of a type held; `None` for a scheme.
    pub fn as_type(self) -> Option<Interval> {
        match self {
            Holding::Type(typed) => Some(typed),
            Holding::Scheme(..) => None,
        }
    }

    /// This holding laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Holding<'to> {
        match *self {
            Holding::Type(typed) => Holding::Type(typed),
            Holding::Scheme(scheme, rebounds) => Holding::Scheme(scheme, rebounds.laid(lay)),
        }
    }
}

/// A callable's type or shape as a question judges it: a type, or a scheme beside the bounds its
/// group writes over names ([`Rebounds`]), which a binding closes before any door reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shaped<'x> {
    pub ty: DeclaredType<Parametric>,
    pub rebounds: Rebounds<'x>,
}

impl<'x> From<DeclaredType<Parametric>> for Shaped<'x> {
    fn from(ty: DeclaredType<Parametric>) -> Self {
        Shaped {
            ty,
            rebounds: Rebounds(&[]),
        }
    }
}

impl<'x> From<Parametric> for Shaped<'x> {
    fn from(ty: Parametric) -> Self {
        DeclaredType::Type(ty).into()
    }
}

impl<'x> Shaped<'x> {
    /// Hand `each` the type and every bound written over names.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        each(self.ty);
        for (_, bound) in self.rebounds.0 {
            each((*bound).into());
        }
    }

    /// This operand laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Shaped<'to> {
        Shaped {
            ty: self.ty,
            rebounds: self.rebounds.laid(lay),
        }
    }

    /// This operand with every type mapped by `declared`, its bounds laid through `lay`.
    pub fn mapped<'to>(
        &self,
        lay: impl Lays<'to>,
        declared: &mut dyn FnMut(DeclaredType<Parametric>) -> DeclaredType<Parametric>,
    ) -> Shaped<'to> {
        let bounds = self.rebounds.0;
        let ty = declared(self.ty);
        let mut each = |(at, bound): (u8, Parametric)| match declared(bound.into()) {
            DeclaredType::Type(bound) => (at, bound),
            DeclaredType::Scheme(_) => unreachable!("a bound is a type"),
        };
        Shaped {
            ty,
            rebounds: match bounds {
                [] => Rebounds(&[]),
                _ => Rebounds(lay.fill(bounds.len(), |at| each(bounds[at]))),
            },
        }
    }

    /// The scheme with each bound written in where every one is closed, its bounds then empty;
    /// else this operand as it is.
    pub fn rebuilt(self, types: &Chain<'_, '_, '_, '_>, scratch: BumpAllocator<'x>) -> Shaped<'x> {
        if self.rebounds.is_empty() {
            return self;
        }
        let mut closed = BumpVec::with_capacity_in(self.rebounds.0.len(), scratch);
        for (at, bound) in self.rebounds.0 {
            match types.concrete(*bound) {
                Some(bound) => closed.push((usize::from(*at), bound)),
                None => return self,
            }
        }
        match rebuilt_scheme(types, scratch, self.ty, &closed) {
            Some(ty) => ty.into(),
            None => self,
        }
    }
}

/// `scheme` with the bound of each group variable `bounds` names replaced by the closed bound
/// beside it: its body read at fresh quantified variables under the new bounds, bound again under
/// its own group. `None` where `scheme` is no scheme over a function type or a shape.
pub fn rebuilt_scheme(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    scheme: DeclaredType<Parametric>,
    bounds: &[(usize, KType)],
) -> Option<DeclaredType<Parametric>> {
    use crate::type_lattice::instantiate_quantified;
    let DeclaredType::Scheme(scheme) = scheme else {
        return None;
    };
    let group = scheme.group(types);
    let mut fresh = BumpVec::with_capacity_in(group.bounds.len(), scratch);
    fresh.extend(group.bounds.iter().copied());
    for (index, bound) in bounds {
        *fresh.get_mut(*index)? = *bound;
    }
    let mut variables = BumpVec::with_capacity_in(fresh.len(), scratch);
    variables.extend((fresh.iter().enumerate()).map(|(at, bound)| types.quantified(at, *bound)));
    let body = instantiate_quantified(types, scratch, scheme, &variables);
    let rebuilt = match types.node(body) {
        TypeNode::KFunction { params, ret } => {
            let mut fields = BumpVec::new_in(scratch);
            fields.extend(params.iter());
            types.function_scheme(scratch, group.names, &fresh, &fields, ret)
        }
        TypeNode::ExpressionShape {
            elements,
            classes,
            ret,
        } => {
            let mut written = BumpVec::new_in(scratch);
            written.extend(elements.iter());
            let mut ranks = BumpVec::new_in(scratch);
            ranks.extend(classes.iter());
            types.shape_scheme(scratch, group.names, &fresh, &written, &ranks, ret)
        }
        _ => return None,
    };
    Some(rebuilt.handle)
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

/// What a [too wide](Elaboration::TooWide) quantified type holds too many of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Names,
    Parameters,
    Slots,
}

/// The most `FOR ALL` names, parameters or slots a quantified type holds: a stored solve
/// ([`Rule::Solved`](crate::scope::Rule::Solved)) names each by one bit of a `u64`.
pub const WIDEST: usize = 63;

/// The witness that a type was left unknown where no binding is written in: outside a walk, where
/// a namer names no derived name, or past a reader's chain, where a path's root lands at a name no
/// level names ([`Shapes::path_level`](super::Shapes::path_level)). A walk's own namer is total
/// (its error is [`Infallible`](std::convert::Infallible)), so a walk leaves no type unknown over
/// a name a binding writes in. Only those two minters construct one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Unwalked(());

impl Unwalked {
    /// The token, for a minter named in the type's doc.
    pub(crate) const fn outside_a_walk() -> Self {
        Unwalked(())
    }
}

impl From<std::convert::Infallible> for Unwalked {
    fn from(never: std::convert::Infallible) -> Self {
        match never {}
    }
}

/// A path a walk's reader met before the walk resolved it: the path `index` of
/// [`BodyShape::spelled`](super::BodyShape::spelled) of the shape at `segment`. The walk resolves
/// it and types again, so a walk never reads a path it holds no resolution for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unresolved {
    pub segment: Segment,
    pub index: u32,
}

/// Why a type expression did not elaborate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elaboration {
    /// The type name at `site` is bound to something other than a type.
    NotAType { name: TypeSymbol, site: Site },
    /// A spelling at `site` over `name`, a name a run binds, that its bound, written as `written`
    /// where the message can name it so, does not license at every binding
    /// ([what a body may ask of a `FOR ALL` name](../../design/quantified-types.md#what-a-body-may-ask-of-a-for-all-name)).
    Unlicensed {
        name: Parametric,
        written: Option<AsWritten>,
        spelling: Spelling,
        site: Site,
    },
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
    /// A quantified function type or expression shape at `site` with `count` `FOR ALL` names,
    /// parameters or slots (`width`), past what a stored solve holds: it names each by a bit.
    TooWide {
        width: Width,
        count: usize,
        site: Site,
    },
    /// The type declaration of `name`, at `site`, names itself through signatures and aliases
    /// alone: neither mints an identity, so the cycle has no finite type.
    CycleWithoutNominal { name: TypeSymbol, site: Site },
    /// The load-time reader cannot know the type at `site` before the program runs, where no
    /// binding is written in (`unwalked`): the load pass's cue to leave it for the run, never
    /// reported.
    Unknown { site: Site, unwalked: Unwalked },
    /// A `FOR ALL` bound inside a type at `site` reads a run-bound name: no type written over names
    /// holds a scheme whose bound a binding decides. A callable's own group may, its type carrying
    /// the bound beside its scheme ([`Rebounds`]).
    BoundOverRunBound { site: Site },
    /// A type at `site` reads the name `name`, which no layer the run reads binds: what a rigid
    /// fact reaching the run refuses as, worded as the load words a name with no binding.
    Unbound { name: Symbol, site: Site },
    /// A type at `site` the load left open reached the run with no name to word it by: a fact no
    /// walk closed, which debug builds stop at.
    OpenAtRun { site: Site },
    /// A type at `site` reads `path`, which the walk has not resolved yet: its cue to resolve it
    /// and type again, never reported.
    Unresolved { site: Site, path: Unresolved },
}

impl Elaboration {
    /// The site the refusal is about.
    pub fn site(&self) -> Site {
        match self {
            Elaboration::NotAType { site, .. }
            | Elaboration::Unlicensed { site, .. }
            | Elaboration::Unsupported { site }
            | Elaboration::NoSuchMember { site, .. }
            | Elaboration::Bound { site }
            | Elaboration::Quantified { site }
            | Elaboration::MeetOverVariable { site }
            | Elaboration::TiedUnion { site, .. }
            | Elaboration::TooWide { site, .. }
            | Elaboration::CycleWithoutNominal { site, .. }
            | Elaboration::Unknown { site, .. }
            | Elaboration::BoundOverRunBound { site }
            | Elaboration::OpenAtRun { site }
            | Elaboration::Unresolved { site, .. }
            | Elaboration::Unbound { site, .. } => *site,
        }
    }

    /// The refusal as a message, with its names spelled through `symbols` and its
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

/// What an [`Elaboration::Unlicensed`] spelling reads off its run-bound name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spelling {
    /// A field projection `Elt.size` whose bound declares no such field.
    Field(Symbol),
    /// A variant read `Elt.None`, where the bound is a union declaring the variant.
    Variant(Symbol),
    /// An application, a `WITH`, a `NEEDING` or an `UNBUILT` with the name as its head.
    Head,
}

/// A type as the program wrote it, where a message names it so: a type name, or the source of a
/// type expression.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AsWritten {
    Name(TypeSymbol),
    Source(SourceRef),
}

/// The bound a variable's node carries, which a refusal names it by: `ty` itself for a type that
/// is no variable.
fn variable_bound(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> Parametric {
    match types.node(ty) {
        TypeNode::Lexical { bound, .. }
        | TypeNode::Path { bound, .. }
        | TypeNode::Derived { bound, .. }
        | TypeNode::Projection { bound, .. } => bound.into(),
        _ => ty,
    }
}

/// Write `written` as the program spelled it, else `ty` as the lattice renders it.
pub fn written_or(
    f: &mut fmt::Formatter<'_>,
    written: Option<AsWritten>,
    ty: impl Into<DeclaredType<Parametric>>,
    (symbols, types): (&SymbolInterner, &Chain<'_, '_, '_, '_>),
) -> fmt::Result {
    match written {
        Some(AsWritten::Name(name)) => write!(f, "{}", symbols.display(name.symbol())),
        Some(AsWritten::Source(source)) => write!(f, "{}", source.text()),
        None => write!(f, "{}", display_name(ty, types, symbols)),
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
            Elaboration::Unlicensed {
                name,
                written,
                spelling,
                ..
            } => {
                let bound = variable_bound(self.types, *name);
                let name = display_name(*name, self.types, self.symbols);
                let written = (*written, bound);
                let bound = |f: &mut fmt::Formatter<'_>| {
                    written_or(f, written.0, written.1, (self.symbols, self.types))
                };
                match spelling {
                    Spelling::Field(member) => {
                        write!(
                            f,
                            "{name} has no member {}: its bound ",
                            self.symbols.display(*member)
                        )?;
                        bound(f)?;
                        f.write_str(" declares none")
                    }
                    Spelling::Variant(member) => {
                        let member = self.symbols.display(*member);
                        write!(
                            f,
                            "{name} has no member {member}, since a binding may be a lone \
                             variant; spell it "
                        )?;
                        bound(f)?;
                        write!(f, ".{member}")
                    }
                    Spelling::Head => write!(
                        f,
                        "{name} is bound where the program runs, and the type a value carries \
                         is always applied, so it is no head"
                    ),
                }
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
            Elaboration::TooWide { width, count, .. } => write!(
                f,
                "this quantified type has {count} {}, but one holds at most {WIDEST} `FOR ALL` \
                 names and {WIDEST} parameters or slots, so the solve of a call can be stored",
                match width {
                    Width::Names => "`FOR ALL` names",
                    Width::Parameters => "parameters",
                    Width::Slots => "slots",
                }
            ),
            Elaboration::CycleWithoutNominal { name, .. } => write!(
                f,
                "`{}` names itself through signatures and aliases alone, so its type is \
                 infinite; a recursive type passes through a NEWTYPE or a UNION",
                self.symbols.display(name.symbol())
            ),
            Elaboration::BoundOverRunBound { .. } => f.write_str(
                "a `FOR ALL` bound inside a type names a type bound where the program runs, so \
                 each binding would make it a different type; only the `FOR ALL` group of a \
                 callable itself may name one",
            ),
            Elaboration::Unknown { .. }
            | Elaboration::OpenAtRun { .. }
            | Elaboration::Unresolved { .. } => {
                f.write_str("this type is known only where it runs")
            }
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
pub enum Variable {
    /// A type name's lexical variable at `level`: `at` is the coordinate, in the shape the value
    /// was typed in, that holds its type.
    Type { level: usize, at: Coordinate },
    /// A path class's variable, read through the path `entry` of the chain the body `root` roots:
    /// `at` is the coordinate, in the shape the value was typed in, of the module the path's root
    /// names — `None` for a root written in place.
    Path {
        class: PathClass,
        at: Option<Coordinate>,
        root: Segment,
        entry: PathName,
    },
}

impl Variable {
    /// Whether this is `other`'s variable, wherever each is read.
    pub fn names_alike(&self, other: &Variable) -> bool {
        match (self, other) {
            (Variable::Type { level, .. }, Variable::Type { level: theirs, .. }) => level == theirs,
            (Variable::Path { class, .. }, Variable::Path { class: theirs, .. }) => class == theirs,
            _ => false,
        }
    }
}

/// A path entry's name along its chain: a path the chain's types spell, at the level the shape
/// numbers it at, or one the walk mints, by a digest of its root, member chain, label and
/// application ([`PathEntry::minted`]). Every walk of a chain names a path alike.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PathName {
    Spelled(usize),
    Minted([u64; 2]),
}

/// One path of a chain — a type member `label` read through a module-valued expression — by its
/// [`PathName`] ([design/modules.md § Carriers and paths](../../design/modules.md#carriers-and-paths)).
/// The shape numbers one per distinct root, member chain and label its type expressions spell,
/// and the walk mints one per path it pins a module's type through or roots a class's variable
/// at, named by its content. The walk resolves each one in the unit that reads it first
/// ([`LoadFacts::resolution`](super::LoadFacts::resolution)).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PathEntry<'graph> {
    pub root: Root,
    /// The value members read from the root, in order, before `label`.
    pub chain: &'graph [ValueSymbol],
    pub label: TypeSymbol,
    pub name: PathName,
    /// The application the path is read through: the module `chain` reaches is fitted to it, and
    /// the path is what it fits `label` at. Over the chain's lexical variables, which a layer
    /// binding them closes. `None` where the path reads the module's own type member.
    pub through: Option<Parametric>,
}

impl PathEntry<'_> {
    /// The name the walk mints a path no type spells by: a digest of its root, member chain, label
    /// and application.
    pub fn minted(
        root: Root,
        chain: &[ValueSymbol],
        label: TypeSymbol,
        through: Option<Parametric>,
    ) -> PathName {
        let mut hasher = DigestHasher::new(Tag::PathEntry);
        hasher.feed((root, chain, label, through.map(|through| through.digest())));
        PathName::Minted(words(hasher.finished().bits()))
    }
}

impl fmt::Debug for PathEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathEntry")
            .field("name", &self.name)
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
    /// The signature the root holds at every run, written over names, where the path may read no
    /// type member off it: a run that finds none refuses the path, naming it.
    pub lacks: Option<Parametric>,
}

impl Resolution<'_> {
    /// The type the path is at every run, where the load resolved it to one.
    pub fn closed(&self) -> Option<KType> {
        match self.resolved {
            Resolved::Closed(closed) => Some(closed),
            Resolved::Over(_) | Resolved::Class(_) => None,
        }
    }
}

/// The type a path is at load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolved {
    /// The same type at every run.
    Closed(KType),
    /// A type written over run-bound names, which a binding closes: a pin over a name read where
    /// the root carries its application at every run.
    Over(Parametric),
    /// The variable of this class: the paths read at one label, through one application, off
    /// trees of one identity. Every path of one class is one type at every run.
    Class(PathClass),
}

impl Resolved {
    /// The class of the paths read at `label` through `through` off a tree of `identity`: `None`
    /// where the tree holds a leaf no binder names, whose path is like no other.
    pub fn class_of(
        label: TypeSymbol,
        through: Option<Parametric>,
        identity: Identity,
    ) -> Option<PathClass> {
        if identity == Identity::Unnamed {
            return None;
        }
        let mut hasher = DigestHasher::new(Tag::PathClass);
        hasher.feed((label, through.map(|through| through.digest()), identity));
        Some(PathClass(words(hasher.finished().bits())))
    }

    /// The class of the one path `entry` of the chain the body `root` roots, alone in its class.
    pub fn own_class(root: Segment, entry: PathName) -> PathClass {
        let mut hasher = DigestHasher::new(Tag::PathClass);
        hasher.feed((root, entry));
        PathClass(words(hasher.finished().bits()))
    }
}

/// A digest's bits as two words, low first.
fn words(bits: u128) -> [u64; 2] {
    [bits as u64, (bits >> 64) as u64]
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
/// group; or, where the value read is the member `name` of a module the load cannot see, the
/// type that module carries and the type the read is wanted at, written over names, which the
/// frame's binding closes and instances the member at
/// ([`member_instance_closed`](super::application::member_instance_closed)).
#[derive(Clone, Copy, Debug)]
pub enum InstanceRead<'graph> {
    Solved(StaticInstance<'graph>),
    AsIs,
    Member {
        name: ValueSymbol,
        carried: Parametric,
        wanted: MemberWanted,
    },
}

/// The type a member of a module the load cannot see is wanted at where it is read.
#[derive(Clone, Copy, Debug)]
pub enum MemberWanted {
    /// Nothing is wanted of it.
    Nothing,
    /// The type the text wants it at, written over names.
    At(Parametric),
    /// It is the instance argument `argument`, among the member arguments, of the keyworded use
    /// at `site`, whose candidates want it at different types: the type the candidate the
    /// selection's answer runs wants it at, or the type every candidate the answer keeps agrees
    /// on ([`Selectable::wanted`](super::site::Selectable::wanted)).
    OfUse { site: Site, argument: u16 },
}

impl MemberWanted {
    /// The type the text wants the member at, where it names one.
    pub fn at(self) -> Option<Parametric> {
        match self {
            MemberWanted::At(wanted) => Some(wanted),
            _ => None,
        }
    }
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
        let classes = Self::classes(types, shapes);
        let mut outranks = |class, a: usize, b: usize| {
            ranked();
            outranks(types, scratch, shapes[a], shapes[b], class)
        };
        Self::laid(lay, shapes.len(), classes, &mut outranks, builtin)
    }

    /// The table of `shapes`, registered shapes under one key and one ranking, each pair's answer
    /// `asked` of `(class, a, b)`, laid through `lay`: `None` where some pair is left unanswered,
    /// every pair asked still, or past [`MOST`](Self::MOST).
    pub fn asked(
        lay: impl Lays<'graph>,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        shapes: &[DeclaredType<Parametric>],
        builtin: impl Fn(usize) -> bool,
        asked: &mut dyn FnMut(u16, usize, usize) -> Option<bool>,
    ) -> Option<Self> {
        let (len, classes) = (shapes.len(), Self::classes(types, shapes));
        if len > Self::MOST {
            return None;
        }
        let mut answers = BumpVec::with_capacity_in(classes * len * len, scratch);
        let mut answered = true;
        for class in 0..classes {
            let class = u16::try_from(class).expect("a shape has fewer than 65536 slots");
            for a in 0..len {
                for b in 0..len {
                    let answer = (a != b).then(|| asked(class, a, b)).flatten();
                    answered &= a == b || answer.is_some();
                    answers.push(answer == Some(true));
                }
            }
        }
        let at = |class: u16, a: usize, b: usize| answers[(usize::from(class) * len + a) * len + b];
        answered
            .then(|| Self::laid(lay, len, classes, at, builtin))
            .flatten()
    }

    /// How many classes `shapes`' ranking has: only the ranking is read, which a scheme's body
    /// spells as a type's does.
    pub fn classes(types: &Chain<'_, '_, '_, '_>, shapes: &[DeclaredType<Parametric>]) -> usize {
        shapes
            .first()
            .map_or(0, |shape| match shape.view(types).body {
                TypeNode::ExpressionShape { classes, .. } => classes
                    .iter()
                    .max()
                    .map_or(0, |highest| usize::from(highest) + 1),
                _ => unreachable!("a registered shape is an expression shape"),
            })
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
    /// Where the text does not select one candidate, what an answer selects among, which the
    /// use's content tree is keyed on ([`Called`](super::Called)).
    pub undecided: Option<&'graph Selection<'graph>>,
}

impl UseStatics<'_> {
    /// A use the load fixed nothing about: every candidate kept, every argument read as carried.
    pub const UNFIXED: Self = UseStatics {
        narrowing: Narrowing::Full,
        contributions: &[],
        undecided: None,
    };

    /// Whether the load fixed nothing about the use.
    pub fn is_unfixed(&self) -> bool {
        matches!(self.narrowing, Narrowing::Full)
            && self.contributions.is_empty()
            && self.undecided.is_none()
    }
}

/// What the load fixed about one `MATCH`'s arms, each named by its position in written order: the
/// arms a run selects among, each beside its verdict — *always* or *maybe*, never *never* — how
/// those that admit rank, and the arm the run takes without admitting where the load selected one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arms<'graph> {
    pub kept: &'graph [(u16, Verdict)],
    pub ranking: Elimination<'graph>,
    pub chosen: Option<u16>,
}

/// How a call in its frame body's tail position routes its frame, as the walk fixed it: per
/// candidate, in the order the call reads them — one for a selected use or a call by name.
/// Written only at a tail call of a callable body whose callee the load knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route<'graph> {
    pub per: &'graph [CandidateRoute],
}

/// One candidate's route from a tail call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateRoute {
    /// Whether the candidate's return keeps the frame's contract, where the frame's return and the
    /// candidate's solution are closed: `None` where the call decides.
    pub kept: Option<bool>,
    /// Whether the call is a self tail edge of a body whose name does not escape.
    pub hop: bool,
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

/// What the load fixed about where a body's types intern at run: whether its frame caches.
#[derive(Clone, Copy, Debug)]
pub struct Caching {
    /// Whether the frame body this shape runs in caches: its own class for a callable or a quote's
    /// code, its frame's for a block or a module body.
    pub caches: bool,
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
                .any(|variable| matches!(variable, Variable::Path { .. })),
            _ => false,
        }
    }
}
