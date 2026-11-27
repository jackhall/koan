//! Each native's **type rule**: from what a call's slots hold — each argument's static type, read
//! at both ends, and the names a slot holds — the type each argument needs and the call's return
//! interval.
//!
//! The rule types a call at the load. [`statics`](super::statics) hands it each slot's static type
//! and what the slot holds as written, types a builtin's call at [`typed`]'s return, and drops a
//! candidate an argument's lower end lies outside a need of — the judge's own test,
//! [`lower_end_outside`] — naming the field or member the argument lacks ([`Lacks`]). The run reads
//! no rule: each native reads its operands through [the door](crate::values::Surface), so its value
//! carries its rule's exact return, which the evaluator checks against the static type in debug
//! builds. `FROM` and `ATTR` over a record have rules of their own; every other native takes its
//! declared slots as its needs and a return at most its declared one. Over a lexical variable whose
//! bound guarantees the field, both read its projection there (`(FIELD f OF Elt)`), which names the
//! variable rather than reading it through its bound.
//!
//! Every rule obeys a law, which `tests::rules` checks for every builtin: over argument intervals
//! within others, its return lies within theirs; handed no names, its return lies around its return
//! over any; and over its declared slots, its return lies under its declared return. A return whose
//! upper end is `Never` lies within every interval.
//!
//! See [README.md § The builtin table](README.md#the-builtin-table).

use crate::elaborate::judge::{self, Derives, Marks};
use crate::elaborate::{SignatureMember, signature_member};
use crate::memory::{BumpAllocator, BumpVec, collect};
use crate::scope::Rule;
use crate::scope::{Answer, Question};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    DeclaredType, Interval, KKind, KType, Parametric, SchemaDraft, SigOrigin, Step, TypeNode,
    shape_return, shape_slots,
};
use crate::values::{Chain, record_type, under, unknown};

use super::builtins::Native;

/// What a rule reads of one slot of a call.
#[derive(Clone, Copy)]
pub(super) struct Given<'x> {
    /// The argument's static type. A bare label's is its code kind.
    pub typed: Interval,
    /// The names the slot holds as written — a label, a one-name quote, or a list of one-name
    /// quotes. `None` where the load cannot read them.
    pub names: Option<&'x [BinderSymbol]>,
}

/// What an argument whose lower end a need drops may lack ([`lacks`]): nothing the native names,
/// what it lacks at every binding, or, its lower end a variable, the first of the names a field
/// or member read reads, in the order written, that its binding declares none of
/// ([`Rule::Lacks`](crate::scope::Rule::Lacks)).
#[derive(Clone, Copy)]
pub(super) enum Lackable<'x> {
    Nothing,
    Rigid(Lacks),
    Named(&'x [BinderSymbol]),
}

/// What an argument whose lower end a need dropped lacks, in the native's own words.
#[derive(Clone, Copy)]
pub(super) enum Lacks {
    /// A record lacking a field the call reads.
    Field(BinderSymbol),
    /// A module's signature lacking the member the call reads.
    Member(BinderSymbol),
}

/// What a rule gives one call.
pub(super) struct Typed<'x> {
    /// The first slot whose argument's lower end lies outside its need, beside what it lacks
    /// where the rule narrowed that need by a name.
    pub dropped: Option<(usize, Option<Lacks>)>,
    /// The call's return with no need checked: what it returns wherever no need drops it.
    pub read: Interval,
    /// Each slot's need, as a binding narrows it.
    pub needs: BumpVec<'x, SlotNeed<'x>>,
}

/// One slot's need: its declared slot, or a narrower one where the gate's question gives its
/// answer.
#[derive(Clone, Copy)]
pub(super) struct SlotNeed<'x> {
    pub need: KType,
    pub narrowed: Option<Narrowed<'x>>,
}

/// A need narrowed where `gate` answers `answer`, beside whether the load's door so answered.
#[derive(Clone, Copy)]
pub(super) struct Narrowed<'x> {
    pub gate: Question<'x>,
    pub answer: Answer,
    pub need: KType,
    pub taken: bool,
}

impl SlotNeed<'_> {
    /// The need the load checks.
    fn checked(&self) -> KType {
        match self.narrowed {
            Some(narrowed) if narrowed.taken => narrowed.need,
            _ => self.need,
        }
    }
}

/// The first argument a need of `native`'s call drops and the call's return, its overload declared
/// as the shape `declared`, over `given`: the load's entry point.
pub(super) fn typed<'x, U>(
    marks: &dyn Derives<Unwalked = U>,
    native: Native,
    declared: KType,
    given: &[Given<'_>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Typed<'x> {
    let needs = needs(marks, native, declared, given, types, scratch);
    let dropped = (given.iter().zip(needs.iter()))
        .position(|(given, need)| {
            judge::lower_end_outside(marks, types, scratch, given.typed, need.checked())
        })
        .map(|slot| match lacks(native, given, slot, types) {
            Lackable::Rigid(lacks) => (slot, Some(lacks)),
            Lackable::Nothing | Lackable::Named(_) => (slot, None),
        });
    let read = returns(marks, native, declared, given, types, scratch);
    Typed {
        dropped,
        read,
        needs,
    }
}

/// The return of `native`'s call over `given`, with no need checked.
fn returns<U>(
    marks: &dyn Derives<Unwalked = U>,
    native: Native,
    declared: KType,
    given: &[Given<'_>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Interval {
    debug_assert_eq!(given.len(), shape_slots(declared, types).count());
    match (native, named(given, native)) {
        (Native::Project, Some(names)) => {
            let record = given[1].typed;
            let names = distinct(names, scratch);
            let never = |_| KType::NEVER.into();
            Interval {
                lower: projection(
                    marks,
                    types,
                    scratch,
                    &names,
                    |name| lower_field(marks, types, scratch, record.lower, name),
                    never,
                ),
                upper: projection(
                    marks,
                    types,
                    scratch,
                    &names,
                    |name| upper_field(marks, types, scratch, record.upper, name),
                    |record| record,
                ),
            }
        }
        // An operand that never arrives reads nothing.
        (Native::ModuleMember, Some(_))
            if judge::is(marks, types, scratch, given[0].typed.upper, KType::NEVER) =>
        {
            Interval::point(KType::NEVER.into())
        }
        (Native::ModuleMember, Some([name])) => {
            let operand = given[0].typed.upper;
            let read = match signature_member(marks, types, scratch, operand, *name) {
                Some(SignatureMember {
                    declared: DeclaredType::Type(declared),
                    unpinned: None,
                }) => match name {
                    // A type member's value is a type value of its declared type's kind.
                    BinderSymbol::Type(_) => {
                        Interval::point(judge::kind_value(marks, types, scratch, declared))
                    }
                    _ => member_read(marks, types, scratch, given[0].typed, declared),
                },
                _ => match name {
                    BinderSymbol::Type(_) => under(KType::ANY_TYPE.into()),
                    _ => unknown(),
                },
            };
            judge::arrives(marks, types, scratch, &[operand], read)
        }
        (Native::Field, Some([name])) => {
            let value = given[0].typed;
            let upper = upper_field(marks, types, scratch, value.upper, *name);
            // A union field's value keeps its variant, so both ends must name the field alike.
            let alike = Question::Same {
                a: lower_field(marks, types, scratch, value.lower, *name).into(),
                b: upper.into(),
            };
            let lower = judge::decided_by(
                marks,
                types,
                scratch,
                alike,
                &mut |answer| match answer {
                    Answer::Bool(true) => judge::retyped(marks, types, scratch, upper).lower,
                    _ => KType::NEVER.into(),
                },
                KType::NEVER.into(),
            );
            Interval { lower, upper }
        }
        // Building types code exactly, below the kind it was carried at.
        (Native::Build, _) => under(given[0].typed.upper),
        _ => under(declared_return(declared, types).into()),
    }
}

/// The static type of a value member a module typed `operand` declares at `declared`: exactly
/// where the retype makes it so ([`retyped_to`]) and the operand is exact at a signature type —
/// its node itself, never a variable's bound, since the run reads the member through that very
/// schema and retypes it to `declared` — and at most `declared` otherwise. Its lower end is the
/// type on those two answers, so it is the same at load and under every binding.
pub(super) fn member_read<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    operand: Interval,
    declared: Parametric,
) -> Interval {
    // A signature, an application of one or a meet of them is exactly a type of kind signature.
    let never = || Parametric::from(KType::NEVER);
    let exact = Question::Same {
        a: operand.lower.into(),
        b: operand.upper.into(),
    };
    let at_signature = Question::Kind { ty: operand.upper };
    let lower = judge::decided_by(
        marks,
        types,
        scratch,
        exact,
        &mut |answer| match answer {
            Answer::Bool(true) => judge::decided_by(
                marks,
                types,
                scratch,
                at_signature,
                &mut |kind| match kind {
                    Answer::Kind(KKind::Signature) => {
                        judge::retyped(marks, types, scratch, declared).lower
                    }
                    _ => never(),
                },
                never(),
            ),
            _ => never(),
        },
        never(),
    );
    Interval {
        lower,
        upper: declared,
    }
}

/// What the argument at `slot` of `native`'s call may lack, where a need the rule narrowed by the
/// names the call reads drops it ([`Lackable`]): the first name, in the order written, a record
/// lower end lacks, or the one member a module read names. A lower end that is a variable lacks
/// what its binding lacks, a record its field or a signature its member.
pub(super) fn lacks<'x>(
    native: Native,
    given: &[Given<'x>],
    slot: usize,
    types: &Chain<'_, '_, '_, '_>,
) -> Lackable<'x> {
    let Some(names) = named(given, native) else {
        return Lackable::Nothing;
    };
    let read = matches!(
        (native, slot),
        (Native::Project, 1) | (Native::Field, 0) | (Native::ModuleMember, 0)
    );
    let Some(node) = judge::node_unless_variable(types, given[slot].typed.lower) else {
        return match read {
            true => Lackable::Named(names),
            false => Lackable::Nothing,
        };
    };
    let lacks = match (native, slot, node) {
        (Native::Project, 1, TypeNode::Record { fields })
        | (Native::Field, 0, TypeNode::Record { fields }) => (names.iter())
            .find(|name| fields.get(name.symbol()).is_none())
            .map(|name| Lacks::Field(*name)),
        (
            Native::ModuleMember,
            0,
            TypeNode::Signature { .. }
            | TypeNode::SignatureApply { .. }
            | TypeNode::SignatureMeet { .. },
        ) => match names {
            [name @ (BinderSymbol::Value(_) | BinderSymbol::Type(_))] => Some(Lacks::Member(*name)),
            _ => None,
        },
        _ => None,
    };
    lacks.map_or(Lackable::Nothing, Lackable::Rigid)
}

/// The type each slot of `native`'s call needs: its declared slot, narrowed by a rule of its own
/// to a record holding each name the call reads. A narrowing a binding may decide otherwise keeps
/// its gate, the question it rests on.
///
/// A need narrows only at a builtin expression shape's key, which no user overload joins: a
/// candidate a need drops then leaves no other for the run to select, so the load refuses rather
/// than select what full selection would not. Narrowing one at a shadowable key — `==`, `PRINT` —
/// would break that agreement.
fn needs<'x>(
    marks: &dyn Marks,
    native: Native,
    declared: KType,
    given: &[Given<'_>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, SlotNeed<'x>> {
    let mut needs = BumpVec::with_capacity_in(given.len(), scratch);
    needs.extend(shape_slots(declared, types).map(|need| SlotNeed {
        need,
        narrowed: None,
    }));
    let holding = |names: &[BinderSymbol]| {
        record_type(types, scratch, names.iter().map(|name| (*name, KType::ANY)))
    };
    match (native, named(given, native)) {
        (Native::Project, Some(names)) => needs[1].need = holding(&distinct(names, scratch)),
        // Over a lower end no record, `ATTR` reads a type, a tagged value or a module, and faults
        // on its own. Over a module it never reads a field, so a record holding the label is what
        // it needs, which no module is. A variable whose bound guarantees the field has it at
        // every binding, so it needs nothing narrower and asks nothing.
        (Native::Field, Some(names @ [name]))
            if judge::projection(types, scratch, given[0].typed.lower, Step::Field(*name))
                .is_none() =>
        {
            let lower = given[0].typed.lower;
            let (answered, gate) = fielded(marks, types, scratch, lower);
            needs[0].narrowed = (answered != Some(false)).then(|| Narrowed {
                gate,
                answer: Answer::Bool(true),
                need: holding(names),
                taken: answered == Some(true),
            });
        }
        // Over a lower end that is a signature, a module naming the label is what a read needs.
        (Native::ModuleMember, Some([name])) => {
            let lower = given[0].typed.lower;
            let kind = judge::kind_decided(marks, types, scratch, lower);
            if kind.is_some_and(|kind| kind != KKind::Signature) {
                return needs;
            }
            let mut draft = SchemaDraft::new(scratch);
            draft.origin = SigOrigin::Declared;
            match name {
                BinderSymbol::Value(value) => draft.insert_value_slot(*value, KType::ANY),
                BinderSymbol::Type(held) => {
                    draft.insert_parameter(*held, types.head_parameter(*held, KType::ANY));
                }
                BinderSymbol::Registration(_) | BinderSymbol::Key(_) => return needs,
            }
            needs[0].narrowed = Some(Narrowed {
                gate: Question::Kind { ty: lower },
                answer: Answer::Kind(KKind::Signature),
                need: types.signature(scratch, draft),
                taken: kind.is_some(),
            });
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
        Native::Field | Native::ModuleMember => given[1].names,
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
/// since no record holds a field at `Never` ([`judge::never_unless`]). `undecided` is the end
/// outside a walk.
fn projection<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    names: &[BinderSymbol],
    mut field: impl FnMut(BinderSymbol) -> Parametric,
    undecided: impl FnOnce(Parametric) -> Parametric,
) -> Parametric {
    let mut fields = BumpVec::with_capacity_in(names.len(), scratch);
    fields.extend(names.iter().map(|name| (*name, field(*name))));
    let typed = collect(scratch, fields.iter().map(|(_, typed)| *typed));
    let record = record_type(types, scratch, fields.into_iter());
    judge::never_unless(marks, types, scratch, typed, record, undecided(record))
}

/// A type above the field `name` of every value under `ktype`, through [`Rule::FieldAbove`]: a
/// record's field type, joined over a union's members; the projection there where a bound
/// guarantees the field; else, over names, the derived name of the field a binding gives. `Any`
/// where a value under `ktype` may hold no such field, and outside a walk over names.
fn upper_field<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ktype: Parametric,
    name: BinderSymbol,
) -> Parametric {
    let operand = [DeclaredType::Type(ktype)];
    (judge::ruled_one(marks, types, scratch, Rule::FieldAbove(name), &operand).ok())
        .flatten()
        .unwrap_or(KType::ANY.into())
}

/// A type under the field `name` of every value above the lower end `ktype`, through
/// [`Rule::FieldUnder`]: a record's field type, the projection there where a bound guarantees
/// the field, else, over names, the derived name of the field a binding gives; `Never` where it
/// names no such field, and outside a walk over names.
fn lower_field<U>(
    marks: &dyn Derives<Unwalked = U>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ktype: Parametric,
    name: BinderSymbol,
) -> Parametric {
    let operand = [DeclaredType::Type(ktype)];
    (judge::ruled_one(marks, types, scratch, Rule::FieldUnder(name), &operand).ok())
        .flatten()
        .unwrap_or(KType::NEVER.into())
}

/// Whether a value of `lower` names fields — a record or a signature — as [`Rule::Fielded`] asks:
/// every binding's answer, `None` where it is undecided; beside the question.
fn fielded<'x>(
    marks: &dyn Marks,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    lower: Parametric,
) -> (Option<bool>, Question<'x>) {
    let operands = collect(scratch, [DeclaredType::Type(lower)].into_iter());
    let answered = judge::defined(marks, types, scratch, Rule::Fielded, operands).0;
    (
        answered,
        Question::Defined {
            rule: Rule::Fielded,
            operands,
        },
    )
}

/// The return the builtin shape `declared` declares.
fn declared_return(declared: KType, types: &Chain<'_, '_, '_, '_>) -> KType {
    shape_return(declared, types).expect("a builtin's type is an expression shape")
}
