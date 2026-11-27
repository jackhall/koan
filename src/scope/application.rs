//! **An application over a head written over names**: `(head argument)` where the head's static
//! type reads a run-bound name the load cannot see through — `f {x = 1}` over `f :Fn`. What the
//! application is depends on what a binding gives the head: a read where it is a list, a dict, a
//! string or code ([lookup.rs](../dispatch/lookup.rs)), a call by name where it is a function, and
//! unknown otherwise. So its type is a derived name whose term is the whole closed rule, case
//! split included ([`Rule::Application`](super::Rule::Application)): the closed functions here,
//! which the walk runs over a closed head ([lookups.rs](../dispatch/walk/lookups.rs), and `called`
//! in [walk.rs](../dispatch/walk.rs)). A refusal of either is the rule having no answer. A member
//! read off a module the load cannot see may be quantified, which no type written over names
//! holds: its call reads the member off the closed signature, a scheme included, and solves it
//! ([`Rule::MemberApplication`](super::Rule::MemberApplication), [`scheme_call_closed`]); read
//! anywhere else, it is an instance site, solved against the type it is wanted at
//! ([`Rule::MemberInstance`](super::Rule::MemberInstance), [`member_instance_closed`]).

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, Interval, KType, Parametric, Scheme, TypeNode, Verdict,
    instantiate_quantified, is_subtype_of, judge_by_class, lower_end_outside, shape_return,
};
use crate::values::{Chain, record_type, retyped_to, under};

use super::heads::{applied_union, member_declared};

/// What one closed end of a read's head is.
#[derive(Clone, Copy)]
enum Container {
    List(KType),
    Dict { key: KType, value: KType },
    Str,
    Code,
}

/// What the closed `end` is as a read's head: `None` for anything but a list, a dict, `Str` or
/// code.
fn container(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    end: KType,
) -> Option<Container> {
    if end == KType::STR {
        return Some(Container::Str);
    }
    if end != KType::NEVER && is_subtype_of(types, scratch, end, KType::ANY_CODE) {
        return Some(Container::Code);
    }
    match types.node(end) {
        TypeNode::List { element } => Some(Container::List(element)),
        TypeNode::Dict { key, value } => Some(Container::Dict { key, value }),
        _ => None,
    }
}

/// The `from` and `to` bounds a record names, `None` where `end` is no record.
fn bounds(types: &Chain<'_, '_, '_, '_>, end: KType) -> Option<[Option<KType>; 2]> {
    match types.node(end) {
        TypeNode::Record { fields } => {
            let named = |name: &str| {
                let name = crate::symbols::ValueSymbol::classify(name).expect("a value name");
                fields.get(BinderSymbol::Value(name).symbol())
            };
            Some([named("from"), named("to")])
        }
        _ => None,
    }
}

/// Whether a slice's argument whose lower end is the closed `lower` can never read a list or a
/// string: a record naming neither `from` nor `to`, or a bound outside `Number`.
pub fn bounds_refuse(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    lower: KType,
) -> bool {
    bounds(types, lower).is_some_and(|named| {
        named.iter().all(Option::is_none)
            || named.iter().flatten().any(|bound| {
                let bound = Interval::point(Parametric::from(*bound));
                lower_end_outside(types, scratch, bound, Parametric::from(KType::NUMBER))
            })
    })
}

/// The closed read of `head` by `payload`, as the walk's read types it over closed ends: `None`
/// where the head is no container at either end; `Some(None)` where the argument can never read
/// it, a refusal; else its interval, `Result` — the family `result` — at what it reads.
pub fn read_closed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    result: KType,
    head: Interval<KType>,
    payload: Interval<KType>,
) -> Option<Option<Interval<KType>>> {
    let shape = match (
        container(types, scratch, head.upper),
        container(types, scratch, head.lower),
    ) {
        (Some(upper), _) => upper,
        (None, Some(Container::List(_))) => Container::List(KType::ANY),
        (None, Some(Container::Dict { .. })) => Container::Dict {
            key: KType::ANY,
            value: KType::ANY,
        },
        (None, Some(other)) => other,
        (None, None) => return None,
    };
    let interval = Interval {
        lower: Parametric::from(payload.lower),
        upper: Parametric::from(payload.upper),
    };
    let outside = |slot: KType| lower_end_outside(types, scratch, interval, Parametric::from(slot));
    let union = |members: &[KType]| types.union_of(scratch, members);
    let refused = match shape {
        Container::Dict { key, .. } => {
            outside(union(&[KType::STR, KType::NUMBER, KType::BOOL])) || outside(key)
        }
        _ => {
            outside(union(&[KType::NUMBER, KType::EMPTY_RECORD]))
                || bounds_refuse(types, scratch, payload.lower)
        }
    };
    if refused {
        return Some(None);
    }
    let (ok, error) = match shape {
        Container::Dict { key, value } => {
            let error = match is_subtype_of(types, scratch, payload.upper, key) {
                true => key,
                false => union(&[key, payload.upper]),
            };
            (value, error)
        }
        _ => {
            let element = match shape {
                Container::List(element) => element,
                Container::Code => KType::ANY_CODE,
                _ => KType::STR,
            };
            let head_upper = match (shape, container(types, scratch, head.upper)) {
                (Container::Code, _) => KType::ANY_CODE,
                (_, Some(_)) => head.upper,
                (_, None) => KType::ANY,
            };
            let ok = if payload.upper == KType::NUMBER || payload.upper == KType::NEVER {
                element
            } else if bounds(types, payload.upper)
                .is_some_and(|named| named.iter().any(Option::is_some))
            {
                head_upper
            } else {
                union(&[element, head_upper])
            };
            (ok, KType::NUMBER)
        }
    };
    let variant =
        |name: &str| BinderSymbol::Type(TypeSymbol::classify(name).expect("a Type token"));
    let mut arguments = [
        (variant("Ok"), Parametric::from(ok)),
        (variant("Error"), Parametric::from(error)),
    ];
    arguments.sort_unstable_by_key(|(name, _)| *name);
    let upper = types.concrete(applied_union(types, scratch, result, &arguments))?;
    Some(Some(Interval {
        lower: KType::NEVER,
        upper,
    }))
}

/// The record a call by name of the closed function `head` wants its argument at: its parameters,
/// `None` where `head` is no function.
pub fn wanted_closed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    head: KType,
) -> Option<KType> {
    let TypeNode::KFunction { params, .. } = types.node(head) else {
        return None;
    };
    Some(record_type(types, scratch, params.iter()))
}

/// The closed layout of a call by name's argument, of the static type `payload`, onto the
/// parameters `names`: each name's field at each end, `Never` below and `Any` above where that
/// end is no record, in `names`' order. Records are width-superset, so `None` where the lower end
/// is a record lacking a parameter, or the upper end a record naming a field no parameter
/// declares: the call can never name the parameters exactly.
pub fn laid_out_closed<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    names: &[BinderSymbol],
    payload: Interval<KType>,
) -> Option<&'x [Interval<KType>]> {
    let fields = |end: KType| match types.node(end) {
        TypeNode::Record { fields } => Some(fields),
        _ => None,
    };
    let (lower, upper) = (fields(payload.lower), fields(payload.upper));
    let lacks =
        lower.is_some_and(|lower| names.iter().any(|name| lower.get(name.symbol()).is_none()));
    let names_other = upper.is_some_and(|upper| {
        upper
            .keys()
            .any(|field| !names.iter().any(|name| name.symbol() == field.symbol()))
    });
    if lacks || names_other {
        return None;
    }
    let mut laid = BumpVec::with_capacity_in(names.len(), scratch);
    laid.extend(names.iter().map(|name| Interval {
        lower: (lower.and_then(|lower| lower.get(name.symbol()))).unwrap_or(KType::NEVER),
        upper: (upper.and_then(|upper| upper.get(name.symbol()))).unwrap_or(KType::ANY),
    }));
    Some(laid.leak())
}

/// The closed call by name of `head` over `payload`, as the walk types one of a function no group
/// quantifies: `None` where `head` is no function; `Some(None)` where the argument can never name
/// or fit its parameters, a refusal; else its interval: `Never` where an argument never arrives,
/// at most the return of a callee known at most, and else the return, exactly where its retype is
/// a point.
pub fn call_closed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    head: Interval<KType>,
    payload: Interval<KType>,
) -> Option<Option<Interval<KType>>> {
    let TypeNode::KFunction { params, ret } = types.node(head.upper) else {
        return None;
    };
    let never = Interval::point(KType::NEVER);
    if payload.upper == KType::NEVER {
        return Some(Some(never));
    }
    let exact = head.lower == head.upper;
    let at_most = |ret: KType| {
        let at_most = under(ret.into());
        Interval {
            lower: types.concrete(at_most.lower).unwrap_or(KType::NEVER),
            upper: ret,
        }
    };
    let mut names = BumpVec::with_capacity_in(params.len(), scratch);
    names.extend(params.iter().map(|(name, _)| name));
    names.sort_unstable_by_key(|name| name.symbol());
    let Some(laid) = laid_out_closed(types, scratch, &names, payload) else {
        return match exact {
            true => Some(None),
            false => Some(Some(at_most(ret))),
        };
    };
    let mut laid_over = BumpVec::with_capacity_in(laid.len(), scratch);
    laid_over.extend(laid.iter().map(|each| Interval {
        lower: Parametric::from(each.lower),
        upper: Parametric::from(each.upper),
    }));
    let laid = laid_over;
    if laid.iter().any(|each| each.upper == KType::NEVER.into()) {
        return Some(Some(never));
    }
    if !exact {
        return Some(Some(at_most(ret)));
    }
    let mut elements = BumpVec::with_capacity_in(names.len(), scratch);
    let mut sorted = BumpVec::with_capacity_in(params.len(), scratch);
    sorted.extend(params.iter());
    sorted.sort_unstable_by_key(|(name, _)| name.symbol());
    elements.extend(
        sorted
            .iter()
            .map(|(_, param)| DispatchTokenElement::Slot(Parametric::from(*param))),
    );
    let classes = scratch.alloc_slice_fill_copy(sorted.len(), 0u16);
    let shape = types.shape_scheme(scratch, &[], &[], &elements, classes, ret.into());
    if judge_by_class(types, scratch, shape.handle, &laid).verdict == Verdict::Never {
        return Some(None);
    }
    let retyped = retyped_to(types, scratch, ret.into());
    Some(Some(Interval {
        lower: types.concrete(retyped.lower).unwrap_or(KType::NEVER),
        upper: ret,
    }))
}

/// `(head argument)` over the closed `head` and `payload`, `result` the builtin result family: a
/// read where the head is a container, a call by name where it is a function, `[Never, Any]`
/// otherwise. `None` where either refuses.
pub fn applied_closed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    result: KType,
    head: Interval<KType>,
    payload: Interval<KType>,
) -> Option<Interval<KType>> {
    match read_closed(types, scratch, result, head, payload) {
        Some(read) => read,
        None => match call_closed(types, scratch, head, payload) {
            Some(called) => called,
            None => Some(Interval {
                lower: KType::NEVER,
                upper: KType::ANY,
            }),
        },
    }
}

/// What the value member of a closed module type read at an instance site is, solved as a fresh
/// load solves a quantified function read there: one closed function, which the load's question,
/// the read's static type and the run's instance all read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberInstance<'x> {
    /// A member declared at a type: read as it is.
    Plain(KType),
    /// No such member: the member read refuses, as `ATTR` does.
    Undeclared,
    /// A quantified member's instance at `solution`, in group order.
    Instance { ktype: KType, solution: &'x [KType] },
    /// A quantified member whose wanted type fixes none of `variables`: no type is wanted, the
    /// wanted type is no function type, or it reaches none of them.
    Unfixed { variables: &'x [TypeSymbol] },
    /// A quantified member, `scheme`, with no instance under the wanted type.
    NoInstance(Scheme),
}

/// The value member `name` of every module under the closed `carried`, read at an instance site
/// wanted at `wanted`: its instance there, solved as the walk solves an instance site's
/// (`instantiate` in [walk.rs](../dispatch/walk.rs)), or why it has none. A member declared at a
/// type is read as it is.
pub fn member_instance_closed<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    carried: KType,
    name: ValueSymbol,
    wanted: Option<KType>,
) -> MemberInstance<'x> {
    let scheme = match member_declared(types, scratch, carried, name) {
        Some(DeclaredType::Scheme(scheme)) => scheme,
        Some(DeclaredType::Type(read)) => return MemberInstance::Plain(read),
        None => return MemberInstance::Undeclared,
    };
    let names = scheme.group(types).names;
    let Some(wanted) =
        wanted.filter(|wanted| matches!(types.node(*wanted), TypeNode::KFunction { .. }))
    else {
        return MemberInstance::Unfixed {
            variables: scratch.alloc_slice_copy(names),
        };
    };
    let record = crate::type_lattice::instance_under(types, scratch, scheme, wanted);
    let Some(solved) = record.solved else {
        return MemberInstance::NoInstance(scheme);
    };
    if record.reached.contains(&false) {
        let mut unfixed = BumpVec::new_in(scratch);
        unfixed.extend(
            (0..names.len())
                .filter(|at| !record.reached[*at])
                .map(|at| names[at]),
        );
        return MemberInstance::Unfixed {
            variables: unfixed.leak(),
        };
    }
    let ktype = instantiate_quantified(types, scratch, scheme, solved.types);
    let ktype = types
        .concrete(ktype)
        .expect("a closed scheme instanced at closed types is closed");
    MemberInstance::Instance {
        ktype,
        solution: scratch.alloc_slice_copy(solved.types),
    }
}

/// `(member argument)` where `member` is the value member `name` of every module under the closed
/// `carried`, its value's lower end `lower`: a quantified member's call solved
/// ([`scheme_call_closed`]), any other's as [`applied_closed`] applies a head at the member's type.
/// `None` where the call refuses.
pub fn member_applied_closed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    result: KType,
    (lower, carried, name): (KType, KType, crate::symbols::ValueSymbol),
    payload: Interval<KType>,
) -> Option<Interval<KType>> {
    match member_declared(types, scratch, carried, name) {
        Some(DeclaredType::Scheme(scheme)) => scheme_call_closed(types, scratch, scheme, payload)?,
        declared => {
            let upper = declared
                .and_then(DeclaredType::as_type)
                .unwrap_or(KType::ANY);
            applied_closed(types, scratch, result, Interval { lower, upper }, payload)
        }
    }
}

/// The closed call by name of a function of the scheme `scheme` over `payload`, as the walk types a
/// call of a quantified callee: `None` where `scheme` is no function's; `Some(None)` where the
/// argument can never name or fit its parameters, a refusal; else its interval: `Never` where an
/// argument never arrives, and else at most its return at the group's solution, since the
/// function a module holds may be one admitting more.
pub fn scheme_call_closed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
    payload: Interval<KType>,
) -> Option<Option<Interval<KType>>> {
    let view = DeclaredType::<KType>::Scheme(scheme).view(types);
    let TypeNode::KFunction { params, ret } = view.body else {
        return None;
    };
    let never = Interval::point(KType::NEVER);
    if payload.upper == KType::NEVER {
        return Some(Some(never));
    }
    let mut sorted = BumpVec::with_capacity_in(params.len(), scratch);
    sorted.extend(params.iter());
    sorted.sort_unstable_by_key(|(name, _)| name.symbol());
    let mut names = BumpVec::with_capacity_in(sorted.len(), scratch);
    names.extend(sorted.iter().map(|(name, _)| *name));
    let Some(laid) = laid_out_closed(types, scratch, &names, payload) else {
        return Some(None);
    };
    if laid.iter().any(|each| each.upper == KType::NEVER) {
        return Some(Some(never));
    }
    let mut laid_over = BumpVec::with_capacity_in(laid.len(), scratch);
    laid_over.extend(laid.iter().map(|each| Interval {
        lower: Parametric::from(each.lower),
        upper: Parametric::from(each.upper),
    }));
    let mut elements = BumpVec::with_capacity_in(sorted.len(), scratch);
    elements.extend(
        sorted
            .iter()
            .map(|(_, param)| DispatchTokenElement::Slot(*param)),
    );
    let classes = scratch.alloc_slice_fill_copy(sorted.len(), 0u16);
    let shape = types.shape_scheme(
        scratch,
        view.group.names,
        view.group.bounds,
        &elements,
        classes,
        ret,
    );
    let judged = judge_by_class(types, scratch, shape.handle, &laid_over);
    if judged.verdict == Verdict::Never {
        return Some(None);
    }
    let returned = match (shape.handle, judged.solve.intervals()) {
        (DeclaredType::Scheme(shape), Some(intervals)) => {
            let mut solution = BumpVec::with_capacity_in(intervals.len(), scratch);
            solution.extend(intervals.iter().map(|each| each.upper));
            let instance = instantiate_quantified(types, scratch, shape, &solution);
            shape_return(instance, types).and_then(|ret| types.concrete(ret))
        }
        _ => None,
    };
    let upper = returned.unwrap_or(KType::ANY);
    let at_most = under(upper.into());
    Some(Some(Interval {
        lower: types.concrete(at_most.lower).unwrap_or(KType::NEVER),
        upper,
    }))
}
