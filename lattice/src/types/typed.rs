//! The lattice's typed relations: every relation the rest of koan calls, over typed handles. Each
//! wraps the relation of the same name and says, where it wraps a result, why the result keeps its
//! handle's promise.
//!
//! The split is the point. The **order**, `join` and `meet` relate concrete types and nothing else,
//! so they take and return [`KType`]s. *Fits*, the unifier, and the ranking and judging relations
//! read parametric types — a variable by the rigid rule, a scheme by instantiation — so they take
//! [`Parametric`] or [`DeclaredType`] operands. Inside the lattice every relation runs over
//! [`Linked`] nodes: each function here enters its operands once and reads its result back as a
//! handle.

use crate::bump::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, TypeSymbol};

use super::handle::{DeclaredType, Handle, KType, Naming, Parametric, Scheme, TypeHandle};
use super::lattice;
use super::node::Variable;
use super::order;
use super::ranking::{self, Judged};
use super::registry::TypeRegistry;
use super::schema::Members;
use super::shape::Specificity;
use super::sig_relations::{self, FitsFailure, InstanceFailure};
use super::stored::Linked;
use super::substitute::{self, Side};
use super::unify::Interval;

/// Each of `handles`' nodes, entered once into `scratch`.
fn enter_all<'s, 'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    handles: impl ExactSizeIterator<Item = Handle>,
) -> BumpVec<'s, Linked<'run>> {
    let mut nodes = BumpVec::with_capacity_in(handles.len(), scratch);
    nodes.extend(handles.map(|handle| types.enter(handle)));
    nodes
}

/// Whether `a` is below `b` in the one order: reflexive, transitive and antisymmetric over concrete
/// types, which hold no rigid variable. An opaque carrier is an atom of the order.
pub fn is_subtype_of(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: KType,
    b: KType,
) -> bool {
    order::is_subtype_of(types, scratch, a.linked(types), b.linked(types))
}

/// Whether `a` fits `b`: the order, but for the clauses that solve — a scheme through an
/// instance, a signature type through its members — and the rigid rule, under which a variable
/// lies under its bound and above only itself, `Never` and its lower end.
pub fn fits(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: impl Into<DeclaredType<Parametric>>,
    b: impl Into<DeclaredType<Parametric>>,
) -> bool {
    let (a, b) = (types.enter(a.into().raw()), types.enter(b.into().raw()));
    order::fits(types, scratch, a, b)
}

/// Whether the type a position carries fills the slot declared there — *fits*, read from the
/// slot's side.
pub fn satisfied_by(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    slot: impl Into<DeclaredType<Parametric>>,
    carried: impl Into<DeclaredType<Parametric>>,
) -> bool {
    let slot = types.enter(slot.into().raw());
    let carried = types.enter(carried.into().raw());
    order::satisfied_by(types, scratch, slot, carried)
}

/// The least upper bound of two concrete types: a concrete union of concrete members.
pub fn join(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, a: KType, b: KType) -> KType {
    KType::read(lattice::join(
        types,
        scratch,
        a.linked(types),
        b.linked(types),
    ))
}

/// The join of every type `iter` yields; `Never`, the join's identity, for none.
pub fn join_iter(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    iter: impl IntoIterator<Item = KType>,
) -> KType {
    KType::read(lattice::join_iter(
        types,
        scratch,
        iter.into_iter().map(|kt| kt.linked(types)),
    ))
}

/// The greatest lower bound of two concrete types. Every pair it reaches is concrete, so every node
/// it rebuilds is.
pub fn meet(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, a: KType, b: KType) -> KType {
    KType::read(lattice::meet_through_variables(
        types,
        scratch,
        a.linked(types),
        b.linked(types),
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
    let nodes = enter_all(
        types,
        scratch,
        arguments.iter().map(|argument| argument.raw()),
    );
    let solution = ranking::admit_by_class(types, scratch, types.enter(declared.raw()), &nodes)?;
    Some(scratch.alloc_slice_fill_iter(solution.iter().map(|solved| KType::read(solved))))
}

/// Which slots of `declared` its group's solve reads: each naming a variable its own priority class
/// is the first to mention. Empty for anything that is not a shape.
pub fn solving_slots<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: DeclaredType<Parametric>,
) -> &'s [bool] {
    ranking::solving_slots(types, scratch, types.enter(declared.raw()))
}

/// Judge `declared` against one static type per slot, class by class: *never*, *always* or *maybe*,
/// beside each variable's interval.
pub fn judge_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: DeclaredType<Parametric>,
    arguments: &[Interval<Parametric>],
) -> Judged<'s> {
    let mut nodes = BumpVec::with_capacity_in(arguments.len(), scratch);
    nodes.extend(arguments.iter().map(|argument| argument.linked(types)));
    ranking::judge_by_class(types, scratch, types.enter(declared.raw()), &nodes)
}

/// The first two members of a union at a covariant position of `declared` — a binder's parameter,
/// slot or representation, read under its own group of `names` bounded by `bounds` — that tie: both
/// name the group, and neither is strictly more specific while some argument admits both. A binder
/// holding one is refused. Each member comes back with the group's variables as stand-ins named as
/// `names` names them, for the refusal to render: no relation reads it.
pub fn tied_members(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: Parametric,
    names: &[TypeSymbol],
    bounds: &[KType],
) -> Option<(Parametric, Parametric)> {
    let (a, b) = super::unify::tied_members(types, scratch, declared.linked(types))?;
    let mut stand_ins = BumpVec::with_capacity_in(names.len(), scratch);
    stand_ins.extend((names.iter().zip(bounds)).map(|(name, bound)| {
        types.lexical_node(
            scratch,
            ranking::STAND_IN_LEVEL,
            *name,
            types.never(),
            bound.linked(types),
        )
    }));
    let named = |member| {
        Parametric::read(substitute::substitute_quantified(
            types, scratch, member, &stand_ins,
        ))
    };
    Some((named(a), named(b)))
}

/// Whether `argument`'s lower end, read below its variables as [`judge_by_class`] reads it, lies
/// outside `slot`: every type the run can carry there lies outside it too.
pub fn lower_end_outside(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    argument: Interval<Parametric>,
    slot: impl Into<DeclaredType<Parametric>>,
) -> bool {
    let slot = types.enter(slot.into().raw());
    ranking::lower_end_outside(types, scratch, argument.lower.linked(types), slot)
}

/// Whether `a` strictly outranks `b` at `class`, as [`select_by_class`] eliminates by.
pub fn outranks(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: DeclaredType<Parametric>,
    b: DeclaredType<Parametric>,
    class: u16,
) -> bool {
    let (a, b) = (types.enter(a.raw()), types.enter(b.raw()));
    ranking::outranks(types, scratch, a, b, class)
}

/// The survivors of the class-by-class elimination over `shapes`, as indices into `shapes`.
pub fn select_by_class<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    shapes: &[DeclaredType<Parametric>],
) -> BumpVec<'s, usize> {
    let nodes = enter_all(types, scratch, shapes.iter().map(|shape| shape.raw()));
    ranking::select_by_class(types, scratch, &nodes)
}

/// Rank two candidates under one bucket key and ranking, lexicographically by class.
pub fn shape_specificity(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: DeclaredType<Parametric>,
    b: DeclaredType<Parametric>,
) -> Specificity {
    let (a, b) = (types.enter(a.raw()), types.enter(b.raw()));
    sig_relations::shape_specificity(types, scratch, a, b)
}

/// Whether `offered` fits every application `asked` holds — *fits* over two signature types.
pub fn sig_fits<'run: 's, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    offered: impl Into<Parametric>,
    asked: impl Into<Parametric>,
) -> Result<(), FitsFailure<'run, 's>> {
    let offered = offered.into().linked(types);
    let asked = asked.into().linked(types);
    sig_relations::sig_fits(types, scratch, offered, asked)
}

/// `offered` fitted against the one application of `signature` with `pins`, and what that solves
/// each of `signature`'s head parameters to. A solution may hold the offered side's stand-ins for
/// its own unpinned parameters, so it is parametric.
pub fn fits_application<'run: 's, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    offered: KType,
    signature: KType,
    pins: &[(BinderSymbol, KType)],
) -> Result<Members<'s, TypeSymbol, Parametric>, FitsFailure<'run, 's>> {
    let mut nodes = BumpVec::with_capacity_in(pins.len(), scratch);
    nodes.extend(pins.iter().map(|(name, pin)| (*name, pin.linked(types))));
    let (offered, signature) = (offered.linked(types), signature.linked(types));
    let solution = sig_relations::fits_application(types, scratch, offered, signature, &nodes)?;
    Ok(Members::from_pairs(
        scratch,
        solution
            .iter()
            .map(|(name, solved)| (*name, Parametric::read(solved))),
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
    let bindings = enter_all(types, scratch, bindings.iter().map(|binding| binding.raw()));
    Parametric::read(substitute::substitute_quantified(
        types,
        scratch,
        kt.linked(types),
        &bindings,
    ))
}

/// `scheme` at a solved call: its body, substituted through `bindings`, in the scheme's group
/// order. The body binds no group, so it is a [`Parametric`] — concrete where every binding is.
pub fn instantiate_quantified<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
    bindings: &[B],
) -> Parametric {
    let bindings = enter_all(types, scratch, bindings.iter().map(|binding| binding.raw()));
    Parametric::read(substitute::instantiate_quantified(
        types,
        scratch,
        types.enter(scheme.raw()),
        &bindings,
    ))
}

/// *Fits*' instantiation clause for a function scheme, answering the least instance under
/// `wanted`, a function type, in group order: [`InstanceFailure::NoInstance`] where the scheme does
/// not fit `wanted`, and [`InstanceFailure::Unfixed`] where it does but no contribution reaches
/// some variable. Each entry is a contribution's join or meet, so a solution under a `T` is a `T`.
pub fn instance_under<'s, 'run: 's, T: TypeHandle>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    scheme: Scheme,
    wanted: T,
) -> Result<BumpVec<'s, T>, InstanceFailure<'s>> {
    let scheme = types.enter(scheme.raw());
    sig_relations::instance_under(types, scratch, scheme, wanted.linked(types))
}

/// `kt` with every variable replaced by its bound. A parametric type holds no scheme, so every
/// variable in it is free and the result is concrete.
pub fn erase_rigid(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: Parametric) -> KType {
    KType::read(substitute::erase_rigid(types, scratch, kt.linked(types)))
}

/// `kt` read from above with every variable at its two ends — the concrete type above every
/// instance. A parametric type holds no scheme, so every variable in it is read and the result is
/// concrete.
pub fn bound_above(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: Parametric) -> KType {
    KType::read(substitute::bound_above(types, scratch, kt.linked(types)))
}

/// `scheme` read from above, each variable free in it — a lexical variable, a head parameter — at
/// its two ends. Its own group stays bound, so it is still a scheme, and holds no variable outside
/// that group.
pub fn scheme_bound_above(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
) -> Scheme {
    let node = substitute::bound_above(types, scratch, types.enter(scheme.raw()));
    super::handle::wrap(node.handle())
}

/// `kt` read through intervals: each free variable that `interval` answers for replaced by the end
/// `side` takes at its position; one it answers `None` for is kept. The interval `interval` answers
/// is entered where it is handed back.
pub fn read_through(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Parametric,
    side: Side,
    interval: &mut impl FnMut(Variable) -> Option<Interval<Parametric>>,
) -> Parametric {
    Parametric::read(substitute::read_through(
        types,
        scratch,
        kt.linked(types),
        side,
        &mut |_, node| interval(Variable::of(node)?.typed()).map(|ends| ends.linked(types)),
    ))
}

/// What a substitution of lexical levels or head parameters keeps the kind of: a type that may
/// hold a variable stays one, and a scheme keeps its group, since no scheme captures a lexical
/// variable or a head parameter.
pub trait Substitutable: Copy + sealed::Substitutable {}

mod sealed {
    use super::{DeclaredType, Linked, Parametric, Scheme, TypeHandle, TypeRegistry};
    use crate::types::handle::wrap;

    pub trait Substitutable {
        /// `self` rebuilt through `rebuild` over its node, its kind kept.
        fn through<'run>(
            self,
            types: &TypeRegistry<'run>,
            rebuild: impl FnOnce(Linked<'run>) -> Linked<'run>,
        ) -> Self;
    }

    impl Substitutable for Parametric {
        fn through<'run>(
            self,
            types: &TypeRegistry<'run>,
            rebuild: impl FnOnce(Linked<'run>) -> Linked<'run>,
        ) -> Self {
            wrap(rebuild(types.enter(self.raw())).handle())
        }
    }

    impl Substitutable for Scheme {
        fn through<'run>(
            self,
            types: &TypeRegistry<'run>,
            rebuild: impl FnOnce(Linked<'run>) -> Linked<'run>,
        ) -> Self {
            wrap(rebuild(types.enter(self.raw())).handle())
        }
    }

    impl Substitutable for DeclaredType<Parametric> {
        fn through<'run>(
            self,
            types: &TypeRegistry<'run>,
            rebuild: impl FnOnce(Linked<'run>) -> Linked<'run>,
        ) -> Self {
            match self {
                DeclaredType::Type(kt) => DeclaredType::Type(kt.through(types, rebuild)),
                DeclaredType::Scheme(scheme) => {
                    DeclaredType::Scheme(scheme.through(types, rebuild))
                }
            }
        }
    }
}

impl Substitutable for Parametric {}
impl Substitutable for Scheme {}
impl Substitutable for DeclaredType<Parametric> {}

/// `kt` with every lexical variable whose level `bindings` covers replaced by its binding, under
/// any scheme — what a run reads a load-time type through.
pub fn substitute_levels<S: Substitutable>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: S,
    bindings: &[KType],
) -> S {
    let bindings = enter_all(types, scratch, bindings.iter().map(|binding| binding.raw()));
    sealed::Substitutable::through(kt, types, |node| {
        substitute::substitute_levels(types, scratch, node, &bindings)
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
    let bindings = Members::from_pairs(
        scratch,
        bindings
            .iter()
            .map(|(name, binding)| (*name, binding.linked(types))),
    );
    sealed::Substitutable::through(kt, types, |node| {
        substitute::substitute_parameters(types, scratch, node, bindings)
    })
}
