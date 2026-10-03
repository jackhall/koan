//! The lattice's typed relations: every relation the rest of koan calls, over typed handles. Each
//! wraps the raw relation of the same name and says, where it wraps a result, why the result keeps
//! its handle's promise.
//!
//! The split is the point. The **order**, `join` and `meet` relate concrete types and nothing else,
//! so they take and return [`KType`]s. *Fits*, the unifier, and the ranking and judging relations
//! read parametric types — a variable by the rigid rule, a scheme by instantiation — so they take
//! [`Parametric`] or [`DeclaredType`] operands. Inside the lattice every relation runs over raw
//! [`Handle`]s.

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, TypeSymbol};

use super::handle::{DeclaredType, Handle, KType, Parametric, Scheme, TypeHandle, wrap};
use super::lattice;
use super::node::Variable;
use super::order;
use super::ranking::{self, Judged};
use super::registry::TypeRegistry;
use super::schema::Members;
use super::shape::Specificity;
use super::sig_relations::{self, FitsFailure, InstanceFailure};
use super::substitute::{self, Side};
use super::unify::Interval;

/// Whether `a` is below `b` in the one order: reflexive, transitive and antisymmetric over concrete
/// types. A rigid variable reaches the order only as an opaque carrier.
pub fn is_subtype_of(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: KType,
    b: KType,
) -> bool {
    order::is_subtype_of(types, scratch, a.raw(), b.raw())
}

/// Whether `a` fits `b`: the order, but for the clauses that solve — a quantified binder through an
/// instance, a signature type through its members — and the rigid rule, under which a variable
/// lies under its bound and above only itself, `Never` and its lower end.
pub fn fits(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> bool {
    order::fits(types, scratch, a.into().raw(), b.into().raw())
}

/// Whether the type a position carries fills the slot declared there — *fits*, read from the
/// slot's side.
pub fn satisfied_by(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    slot: impl Into<DeclaredType<Parametric>>,
    carried: impl Into<DeclaredType<Parametric>>,
) -> bool {
    order::satisfied_by(types, scratch, slot.into().raw(), carried.into().raw())
}

/// The least upper bound of two concrete types: a concrete union of concrete members.
pub fn join(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, a: KType, b: KType) -> KType {
    wrap(lattice::join(types, scratch, a.raw(), b.raw()))
}

/// The join of every type `iter` yields; `Never`, the join's identity, for none.
pub fn join_iter(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    iter: impl IntoIterator<Item = KType>,
) -> KType {
    wrap(lattice::join_iter(
        types,
        scratch,
        iter.into_iter().map(KType::raw),
    ))
}

/// The greatest lower bound of two concrete types. Every pair it reaches is concrete, so every node
/// it rebuilds is.
pub fn meet(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, a: KType, b: KType) -> KType {
    wrap(lattice::meet_through_variables(
        types,
        scratch,
        a.raw(),
        b.raw(),
    ))
}

/// Solve `declared`'s group against one concrete argument per slot, class by class — what a
/// keyworded call admits by. The solution in the shape's group order; `None` when some class does
/// not admit, when `declared` is not a shape, or when the arguments are not one per slot.
///
/// Every contribution is part of a concrete argument and every bound is concrete, so the solution
/// is.
pub fn admit_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: DeclaredType<KType>,
    arguments: &[KType],
) -> Option<&'s [KType]> {
    let mut raw = BumpVec::with_capacity_in(arguments.len(), scratch);
    raw.extend(arguments.iter().map(|argument| argument.raw()));
    let solution = ranking::admit_by_class(types, scratch, declared.raw(), &raw)?;
    Some(scratch.alloc_slice_fill_iter(solution.iter().map(|solved| wrap(*solved))))
}

/// Which slots of `declared` its group's solve reads: each naming a variable its own priority class
/// is the first to mention. Empty for anything that is not a shape.
pub fn solving_slots<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: DeclaredType<Parametric>,
) -> &'s [bool] {
    ranking::solving_slots(types, scratch, declared.raw())
}

/// Judge `declared` against one static type per slot, class by class: *never*, *always* or *maybe*,
/// beside each variable's interval.
pub fn judge_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: DeclaredType<Parametric>,
    arguments: &[Interval<Parametric>],
) -> Judged<'s> {
    let mut raw = BumpVec::with_capacity_in(arguments.len(), scratch);
    raw.extend(arguments.iter().map(|argument| argument.raw()));
    ranking::judge_by_class(types, scratch, declared.raw(), &raw)
}

/// Whether `a` is at least as specific as `b` at `class`.
pub fn class_at_least(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: DeclaredType<Parametric>,
    b: DeclaredType<Parametric>,
    class: u8,
) -> bool {
    ranking::class_at_least(types, scratch, a.raw(), b.raw(), class)
}

/// The survivors of the class-by-class elimination over `shapes`, as indices into `shapes`.
pub fn select_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    shapes: &[DeclaredType<Parametric>],
) -> BumpVec<'s, usize> {
    let mut raw = BumpVec::with_capacity_in(shapes.len(), scratch);
    raw.extend(shapes.iter().map(|shape| shape.raw()));
    ranking::select_by_class(types, scratch, &raw)
}

/// Rank two candidates under one bucket key and ranking, lexicographically by class.
pub fn shape_specificity(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: DeclaredType<Parametric>,
    b: DeclaredType<Parametric>,
) -> Specificity {
    sig_relations::shape_specificity(types, scratch, a.raw(), b.raw())
}

/// Whether `offered` fits every application `asked` holds — *fits* over two signature types.
pub fn sig_fits<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    offered: impl Into<Parametric>,
    asked: impl Into<Parametric>,
) -> Result<(), FitsFailure<'run, 's>> {
    sig_relations::sig_fits(types, scratch, offered.into().raw(), asked.into().raw())
}

/// `offered` fitted against the one application of `signature` with `pins`, and what that solves
/// each of `signature`'s head parameters to. A solution may hold the offered side's stand-ins for
/// its own unpinned parameters, so it is parametric.
pub fn fits_application<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    offered: KType,
    signature: KType,
    pins: &[(BinderSymbol, KType)],
) -> Result<Members<'s, TypeSymbol, Parametric>, FitsFailure<'run, 's>> {
    let mut raw = BumpVec::with_capacity_in(pins.len(), scratch);
    raw.extend(pins.iter().map(|(name, pin)| (*name, pin.raw())));
    let solution =
        sig_relations::fits_application(types, scratch, offered.raw(), signature.raw(), &raw)?;
    Ok(Members::from_pairs(
        scratch,
        solution.iter().map(|(name, solved)| (*name, wrap(*solved))),
    ))
}

/// Rewrite every free `Quantified(i)` inside `kt` to `bindings[i]`. What a solved call applies to
/// a position it reads under its group.
pub fn substitute_quantified<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Parametric,
    bindings: &[B],
) -> Parametric {
    wrap(substitute::substitute_quantified(
        types,
        scratch,
        kt.raw(),
        bindings,
    ))
}

/// `scheme` at a solved call: opened, each position and the return substituted through
/// `bindings`, in the scheme's group order. The opened callable binds no group, so it is a
/// [`Parametric`] — concrete where every binding is.
pub fn instantiate_quantified<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
    bindings: &[B],
) -> Parametric {
    wrap(substitute::instantiate_quantified(
        types,
        scratch,
        scheme.raw(),
        bindings,
    ))
}

/// *Fits*' instantiation clause for a function scheme, answering the least instance under
/// `wanted`, a function type, in group order: [`InstanceFailure::NoInstance`] where the scheme does
/// not fit `wanted`, and [`InstanceFailure::Unfixed`] where it does but no contribution reaches
/// some variable. Each entry is a contribution's join or meet, so a solution under a `T` is a `T`.
pub fn instance_under<'s, T: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    scheme: Scheme,
    wanted: T,
) -> Result<BumpVec<'s, T>, InstanceFailure<'s>> {
    sig_relations::instance_under(types, scratch, scheme.raw(), wanted.raw())
}

/// `kt` with every variable replaced by its bound. A parametric type holds no binder, so every
/// variable in it is free and the result is concrete.
pub fn erase_rigid(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: Parametric) -> KType {
    wrap(substitute::erase_rigid(types, scratch, kt.raw()))
}

/// `kt` read from above with every variable at its two ends — the concrete type above every
/// instance. A parametric type holds no binder, so every variable in it is read and the result is
/// concrete.
pub fn bound_above(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: Parametric) -> KType {
    wrap(substitute::bound_above(types, scratch, kt.raw()))
}

/// `scheme` read from above, each variable free in it — a lexical variable, a head parameter — at
/// its two ends. Its own group stays bound, so it is still a scheme, and holds no variable outside
/// that group.
pub fn scheme_bound_above(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
) -> Scheme {
    wrap(substitute::bound_above(types, scratch, scheme.raw()))
}

/// `kt` read through intervals: each free variable that `interval` answers for replaced by the end
/// `side` takes at its position; one it answers `None` for is kept.
pub fn read_through(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Parametric,
    side: Side,
    interval: &mut impl FnMut(Variable) -> Option<Interval<Parametric>>,
) -> Parametric {
    wrap(substitute::read_through(
        types,
        scratch,
        kt.raw(),
        side,
        &mut |variable| interval(variable).map(Interval::raw),
    ))
}

/// The bound of each variable of `scheme`'s group, in group order.
pub fn quantifier_bounds<'run>(types: &TypeRegistry<'run>, scheme: Scheme) -> &'run [KType] {
    substitute::quantifier_bounds(types, scheme.raw())
}

/// What a substitution of lexical levels or head parameters keeps the kind of: a type that may
/// hold a variable stays one, and a scheme keeps its group, since no binder captures a lexical
/// variable or a head parameter.
pub trait Substitutable: Copy + sealed::Substitutable {}

mod sealed {
    use super::{DeclaredType, Handle, Parametric, Scheme, TypeHandle, wrap};

    pub trait Substitutable {
        /// `self` rebuilt through `rebuild`, its kind kept.
        fn through(self, rebuild: impl FnOnce(Handle) -> Handle) -> Self;
    }

    impl Substitutable for Parametric {
        fn through(self, rebuild: impl FnOnce(Handle) -> Handle) -> Self {
            wrap(rebuild(self.raw()))
        }
    }

    impl Substitutable for Scheme {
        fn through(self, rebuild: impl FnOnce(Handle) -> Handle) -> Self {
            wrap(rebuild(self.raw()))
        }
    }

    impl Substitutable for DeclaredType<Parametric> {
        fn through(self, rebuild: impl FnOnce(Handle) -> Handle) -> Self {
            match self {
                DeclaredType::Type(kt) => DeclaredType::Type(kt.through(rebuild)),
                DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme.through(rebuild)),
            }
        }
    }
}

impl Substitutable for Parametric {}
impl Substitutable for Scheme {}
impl Substitutable for DeclaredType<Parametric> {}

/// `kt` with every lexical variable whose level `bindings` covers replaced by its binding, under
/// any binder — what a run reads a load-time type through.
pub fn substitute_levels<S: Substitutable>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: S,
    bindings: &[KType],
) -> S {
    sealed::Substitutable::through(kt, |raw| {
        substitute::substitute_levels(types, scratch, raw, bindings)
    })
}

/// `kt` with each head parameter `bindings` names replaced by its binding — how a signature's member
/// types are read under a module's solution, a view's mints or an application's pins.
pub fn substitute_parameters<S: Substitutable, B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: S,
    bindings: Members<'_, TypeSymbol, B>,
) -> S {
    sealed::Substitutable::through(kt, |raw| {
        substitute::substitute_parameters(types, scratch, raw, bindings)
    })
}
