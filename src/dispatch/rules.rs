//! Each native's **type rule**: from what a call's slots hold — each argument's static type, read
//! at both ends, and the names a slot holds — the type each argument needs and the call's return
//! interval.
//!
//! The rule types a call at the load. [`statics`](super::statics) hands it each slot's static type
//! and what the slot holds as written, types a builtin's call at [`typed`]'s return, and drops a
//! candidate an argument's lower end lies outside a need of. The run reads no rule: each native
//! reads its operands through [the door](crate::values::Surface), so its value carries its rule's
//! exact return, which the evaluator checks against the static type in debug builds. `FROM` and
//! `ATTR` over a record have rules of their own; every other native takes its declared slots as its
//! needs and a return at most its declared one.
//!
//! Every rule obeys a law, which `tests::rules` checks for every builtin: over argument intervals
//! within others, its return lies within theirs; handed no names, its return lies around its return
//! over any; and over its declared slots, its return lies under its declared return. A return whose
//! upper end is `Never` lies within every interval.
//!
//! See [README.md § The builtin table](README.md#the-builtin-table).

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    Interval, KType, Parametric, Side, TypeNode, TypeRegistry, bound_above, fits, read_through,
    shape_return, shape_slots,
};
use crate::values::record_type;

use super::builtins::Native;
use super::statics::{retyped_to, under};

/// What a rule reads of one slot of a call.
#[derive(Clone, Copy)]
pub(super) struct Given<'x> {
    /// The argument's static type. A bare label's is its code kind.
    pub typed: Interval,
    /// The names the slot holds as written — a label, a one-name quote, or a list of one-name
    /// quotes. `None` where the load cannot read them.
    pub names: Option<&'x [BinderSymbol]>,
}

/// What a rule gives one call.
pub(super) struct Typed<'x> {
    /// The type each slot's argument needs, slot for slot.
    pub needs: BumpVec<'x, KType>,
    /// The first slot whose argument's lower end lies outside its need.
    pub dropped: Option<usize>,
    /// The call's return: `Never` where a slot is dropped.
    pub returns: Interval,
}

/// The needs and the return of `native`'s call, its overload declared as the shape `declared`,
/// over `given`: the load's entry point.
pub(super) fn typed<'x>(
    native: Native,
    declared: KType,
    given: &[Given<'_>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Typed<'x> {
    let needs = needs(native, declared, given, types, scratch);
    let dropped = (given.iter().zip(needs.iter()))
        .position(|(given, need)| outside(types, scratch, given.typed, *need));
    let returns = match dropped {
        Some(_) => Interval::point(KType::NEVER.into()),
        None => returns(native, declared, given, types, scratch),
    };
    Typed {
        needs,
        dropped,
        returns,
    }
}

/// The return of `native`'s call over `given`, with no need checked.
fn returns(
    native: Native,
    declared: KType,
    given: &[Given<'_>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Interval {
    debug_assert_eq!(given.len(), shape_slots(declared, types).count());
    match (native, named(given, native)) {
        (Native::Project, Some(names)) => {
            let record = given[1].typed;
            let names = distinct(names, scratch);
            Interval {
                lower: projection(types, scratch, &names, |name| {
                    lower_field(types, record.lower, name)
                }),
                upper: projection(types, scratch, &names, |name| {
                    upper_field(types, scratch, record.upper, name)
                }),
            }
        }
        (Native::Field, Some([name])) => {
            let value = given[0].typed;
            let upper = upper_field(types, scratch, value.upper, *name);
            // A union field's value keeps its variant, so both ends must name the field alike.
            if lower_field(types, value.lower, *name) == upper {
                retyped_to(types, upper)
            } else {
                under(upper)
            }
        }
        _ => under(declared_return(declared, types).into()),
    }
}

/// Whether `argument`'s lower end, read below its rigid variables as a candidate's judgement reads
/// it, lies outside `needed`: every type the run can carry there lies outside it too.
fn outside(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    argument: Interval,
    needed: KType,
) -> bool {
    let lower = read_through(
        types,
        scratch,
        argument.lower,
        Side::Below,
        &mut |variable| Some(variable.interval().into()),
    );
    !fits(
        types,
        scratch,
        lower,
        bound_above(types, scratch, needed.into()),
    )
}

/// The type each slot of `native`'s call needs: its declared slot, narrowed by a rule of its own
/// to a record holding each name the call reads.
///
/// A need narrows only at a builtin expression shape's key, which no user overload joins: a
/// candidate a need drops then leaves no other for the run to select, so the load refuses rather
/// than select what full selection would not. Narrowing one at a shadowable key — `==`, `PRINT` —
/// would break that agreement.
fn needs<'x>(
    native: Native,
    declared: KType,
    given: &[Given<'_>],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, KType> {
    let mut needs = BumpVec::with_capacity_in(given.len(), scratch);
    needs.extend(shape_slots(declared, types));
    let holding = |names: &[BinderSymbol]| {
        record_type(types, scratch, names.iter().map(|name| (*name, KType::ANY)))
    };
    match (native, named(given, native)) {
        (Native::Project, Some(names)) => needs[1] = holding(&distinct(names, scratch)),
        // Over a lower end no record, `ATTR` reads a type, a tagged value or a module, and faults
        // on its own.
        (Native::Field, Some(names @ [_]))
            if matches!(types.node(given[0].typed.lower), TypeNode::Record { .. }) =>
        {
            needs[0] = holding(names);
        }
        _ => {}
    }
    needs
}

/// The names `native`'s rule reads — `FROM`'s field list, `ATTR`'s label — where the slot holds
/// them.
fn named<'x>(given: &[Given<'x>], native: Native) -> Option<&'x [BinderSymbol]> {
    match native {
        Native::Project => given[0].names,
        Native::Field => given[1].names,
        _ => None,
    }
}

/// `names`, each once, in the order first listed.
pub(super) fn distinct<'x>(
    names: &[BinderSymbol],
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, BinderSymbol> {
    let mut distinct = BumpVec::with_capacity_in(names.len(), scratch);
    for name in names {
        if !distinct.contains(name) {
            distinct.push(*name);
        }
    }
    distinct
}

/// The record of each of `names` at the type `field` reads for it; `Never` where one reads `Never`,
/// since no record holds a field at `Never`.
fn projection(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    names: &[BinderSymbol],
    mut field: impl FnMut(BinderSymbol) -> Parametric,
) -> Parametric {
    let never = Parametric::from(KType::NEVER);
    let mut fields = BumpVec::with_capacity_in(names.len(), scratch);
    for name in names {
        match field(*name) {
            typed if typed == never => return never,
            typed => fields.push((*name, typed)),
        }
    }
    record_type(types, scratch, fields.into_iter())
}

/// A type above the field `name` of every value under `ktype`: a record's field type, joined over a
/// union's members, read through a rigid variable's bound; `Any` where a value under `ktype` may
/// hold no such field.
fn upper_field(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    ktype: Parametric,
    name: BinderSymbol,
) -> Parametric {
    let node = types.node(ktype);
    if let Some(bound) = node.rigid_bound() {
        return upper_field(types, scratch, bound.into(), name);
    }
    match node {
        TypeNode::Never => KType::NEVER.into(),
        TypeNode::Record { fields } => fields
            .get(name.symbol())
            .unwrap_or_else(|| KType::ANY.into()),
        TypeNode::Union { members } => {
            let mut fields = BumpVec::with_capacity_in(members.len(), scratch);
            fields.extend(
                members
                    .iter()
                    .map(|member| upper_field(types, scratch, member, name)),
            );
            types.union_of(scratch, &fields)
        }
        _ => KType::ANY.into(),
    }
}

/// A type under the field `name` of every value above the lower end `ktype`: a record's field type,
/// or `Never` where it names no such field.
fn lower_field(types: &TypeRegistry<'_>, ktype: Parametric, name: BinderSymbol) -> Parametric {
    match types.node(ktype) {
        TypeNode::Record { fields } => fields
            .get(name.symbol())
            .unwrap_or_else(|| KType::NEVER.into()),
        _ => KType::NEVER.into(),
    }
}

/// The return the builtin shape `declared` declares.
fn declared_return(declared: KType, types: &TypeRegistry<'_>) -> KType {
    shape_return(declared, types).expect("a builtin's type is an expression shape")
}
