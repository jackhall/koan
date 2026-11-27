//! **Typing rules over names** with no case split: each a closed function over operands a run-bound
//! name may reach, giving every type the rule hands back
//! ([build/README.md § Derived names](../build/README.md#derived-names)).
//!
//! A rule hands back the same types at load and under every binding: its closed function's where
//! every operand is closed, else what [`Rule::spelled`] writes over the names, else a
//! [derived name](super::derived) whose term is that output of the closed function. A type that
//! depends on a question's answer is no rule's: it is a [decided](super::DerivedTerm::Decided)
//! derived name, typed on the answers of the question that decides it. A rule's partiality — a
//! piece a type may lack — is such a question, asked beside it.
//!
//! The judge's door over a rule is `judge::ruled`; this module holds the rules themselves, so a
//! derived name's term reaches them from `scope`.

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, NodeSchema, Parametric, Scheme, Step, TypeNode, bound_above,
    class_step, display_name, display_projection, instance_under, judge_by_class, meet,
    scheme_slots,
};
use crate::values::{
    Chain, construction, filled_union, may_hold_record, representation, retyped_to, under,
};

use super::application::{
    MemberInstance, applied_closed, bounds_refuse, laid_out_closed, member_applied_closed,
    member_instance_closed, wanted_closed,
};
use super::heads::{
    applied, closed_variant, constructed_upper, declared_member, head_params, pinned, shown_view,
    signature_lacks, signature_value, tag_union, variants,
};
use super::typed::rebuilt_scheme;

/// A typing rule over operands written over names. Each variant names what it reads off its
/// operands and how many types it hands back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Rule {
    /// The `step` piece of operand 0, read through the projection door, which answers a union by
    /// the union of its members' pieces. One output.
    Piece(Step),
    /// The meet of operands 0 and 1. One output.
    Meet,
    /// The lower end of what a retype to operand 0 re-stamps at: the declared type itself where
    /// the retype is a point, `Never` where it is at most the type, a record type's `Null`-lowered
    /// self where it keeps its `Null` fields ([`retyped_to`]). One output.
    Retyped,
    /// A container literal's element type over its elements' static types, operands in pairs of
    /// ends, `[lower, upper]`: where every pair is exact, the fill of the first `filled` pairs'
    /// points ([`filled_union`]) as both ends; else the union of their lower ends, each that may
    /// hold a record read as `Never`, and the union of their upper ends. The pairs past `filled`
    /// gate exactness only: a dict's keys. Two outputs, the lower end and the upper.
    Fill { filled: u32 },
    /// A quantified callee's group solved by one class step ([`class_step`]), a solve the load
    /// cannot write over the names it reads, stored whole: operand 0 the callee's scheme, then the
    /// type each slot `involved` names (a bit per slot) contributes, in slot order, then the type
    /// each variable `pinned` names (a bit per variable) is pinned to, in order. Over `exact`
    /// contributions where `exact` says so. Output `v` is variable `v` read from above through the
    /// step's solution; the rule has no answer where the step admits no solve. One output per
    /// variable, `variables`.
    Solved {
        involved: u64,
        pinned: u64,
        exact: bool,
        variables: u8,
    },
    /// A candidate's group's intervals as its verdict judges them ([`judge_by_class`]), where a
    /// run may solve its group otherwise: operand 0 the candidate's shape, then each argument's
    /// ends, `[lower, upper]`. Two outputs per variable, `variables`, its interval's lower end and
    /// upper: `[Never, bound]` where the judgement solves none, as a call read through no solve
    /// reads its group.
    Judged { variables: u8 },
    /// A construction of the family operand 0 names over the payload operand 1, solving the
    /// family's parameters from it ([`construction`]): the family applied at them. One output; the
    /// rule has no answer where the payload solves none.
    Constructed,
    /// The instance operand 0, a scheme, takes wanted at operand 1 ([`instance_under`]): each of
    /// its group's variables' types, `variables` outputs. Where `bounded` names group positions (a
    /// bit each), the operands after the second are their bounds as written, in order, which the
    /// scheme is rebuilt with first ([`rebuilt_scheme`]). The rule has no answer where the scheme
    /// has no instance there, or one leaves a variable no contribution reaches.
    InstanceAt { variables: u8, bounded: u64 },
    /// The member operand 0 declares as `name` ([`declared_member`]): a variant of the union under
    /// it, else a field of the record under it. One output; no answer where it declares none. Laid
    /// over names only where operand 0 is a nominal declared over run-bound names, whose
    /// declaration the load checks declares `name`.
    Member(Symbol),
    /// The family operand 0 names applied ([`applied`]): at the arguments operand 1, a record
    /// type, holds by name, or, `sole`, at its lone parameter, operand 1 the argument. One output;
    /// no answer where the head takes no such arguments. Laid over names only where operand 0 is
    /// a family declared over run-bound names, whose parameters the load checks.
    Applied { sole: bool },
    /// The declared signature operand 0 names with the head parameters operand 1, a record type,
    /// names pinned at its fields ([`pinned`]). One output; no answer where it pins none so. Laid
    /// over names only where operand 0 is a signature declared over run-bound names.
    Pinned,
    /// An end of the construction `(head payload)`, operand 0 the head's type, operand 1 the
    /// payload's end: the lower end where `upper` is unset, the constructed type or `Never`; else
    /// the upper end ([`constructed_upper`]), reached through the union operand 2's owner peels to
    /// ([`tag_union`]) where the head is a tag projection, `projected`. One output.
    Construction { upper: bool, projected: bool },
    /// The variant `label` names of the union operand 0, a `MATCH … UNDER` clause
    /// ([`variant_named`]). One output; no answer where it names none. Laid over names only where
    /// the clause is a union declared over run-bound names, whose labels the load checks.
    Variant(Symbol),
    /// The payload of the variant `label` names of the union operand 0: its representation, `Any`
    /// for a variant with none. One output; no answer where `label` names no variant.
    Payload(Symbol),
    /// The payload a `_` arm under the union operand 0 binds, the variants past it the arms' labels
    /// name: at most the union of the representations of the variants none names, `Any` where one
    /// has none ([`under`]). Two outputs, the lower end and the upper; no answer where operand 0
    /// is no union.
    Uncovered,
    /// A type above the field `name` of every value under operand 0 ([`field_above`]): a record's
    /// field type, joined over a union's members, `Never` at `Never`, `Any` where a value may hold
    /// no such field. One output.
    FieldAbove(BinderSymbol),
    /// A type under the field `name` of every value above operand 0, its lower end: a record's
    /// field type, `Never` where it names no such field. One output.
    FieldUnder(BinderSymbol),
    /// Operand 0 itself where a value of it names fields — a record or a signature — so a field
    /// read narrows its need to a record holding the label. One output; no answer otherwise.
    Fielded,
    /// Operand 0 itself where it is a record naming no field `name`, or a signature declaring no
    /// member `name` ([`signature_lacks`]): what a field or member read whose need drops its
    /// candidate names it lacks. One output; no answer otherwise.
    Lacks(BinderSymbol),
    /// The type the signature operand 0 declares its value member `name` at
    /// ([`signature_value`]): a read through a type a binding decides. One output, `Any` where it
    /// declares the member at a scheme; no answer where it declares no such member.
    SignatureValue(ValueSymbol),
    /// `(head argument)` over a head written over names, operands its ends `[lower, upper]`, the
    /// argument's, then the `Result` family: a read where a binding gives a container
    /// ([`read_closed`]), a call by name where it gives a function ([`call_closed`]), else
    /// `[Never, Any]`. Two outputs, the lower end and the upper; no answer where the closed read
    /// or call refuses.
    Application,
    /// `(member argument)` where `member` is the value member `name` of a module the load cannot
    /// see: operands the member's lower end, the type its module carries, the argument's ends, then
    /// the `Result` family. The member is read off the carried signature, a scheme included, and
    /// called ([`member_applied_closed`]); two outputs as [`Rule::Application`]'s.
    MemberApplication(ValueSymbol),
    /// `member` read at an instance site, where `member` is the value member `name` of a module
    /// the load cannot see: operand 0 the type its module carries, then, where `wanted` is set,
    /// the type it is wanted at. The member is read off the carried signature and instanced there
    /// ([`member_instance_closed`]): one output, the instance's type, or the member's own where it
    /// is declared at a type; no answer where a quantified member has no instance there.
    MemberInstance { name: ValueSymbol, wanted: bool },
    /// The record a call by name of operand 0 wants its argument at: a function's parameters, else
    /// `Any` ([`wanted_closed`]). One output.
    Wanted,
    /// The signature an opaque view carries, over its **shown application**, operand 0: the
    /// ascribed application, operand 1, with each head parameter it leaves unpinned pinned at the
    /// path through the view's own tree, which a binding closes to the carrier the view hides it
    /// behind ([`shown_view`]). One output; no answer where a parameter is left unpinned.
    View,
    /// A call by name's argument, operands its ends `[lower, upper]`, laid onto the parameters
    /// operand 2, a record type, names as fields ([`laid_out_closed`]): each parameter's field at
    /// each end, in the record's order. Two outputs per parameter, `slots`, its lower end and
    /// upper; no answer where the call can never name the parameters exactly.
    LaidOut { slots: u8 },
    /// A slice's argument's lower end, operand 0, as a list or a string reads it
    /// ([`bounds_refuse`]): operand 0 itself. One output; no answer where its bounds refuse.
    SliceBounds,
}

impl Rule {
    /// How many types the rule hands back.
    pub fn outputs(&self) -> u8 {
        match self {
            Rule::Piece(_)
            | Rule::Meet
            | Rule::Retyped
            | Rule::Constructed
            | Rule::Member(_)
            | Rule::Applied { .. }
            | Rule::Pinned
            | Rule::Variant(_)
            | Rule::Payload(_)
            | Rule::Construction { .. }
            | Rule::FieldAbove(_)
            | Rule::FieldUnder(_)
            | Rule::Fielded
            | Rule::Lacks(_)
            | Rule::SignatureValue(_)
            | Rule::MemberInstance { .. }
            | Rule::Wanted
            | Rule::View
            | Rule::SliceBounds => 1,
            Rule::Fill { .. }
            | Rule::Uncovered
            | Rule::Application
            | Rule::MemberApplication(_) => 2,
            Rule::Solved { variables, .. } => *variables,
            Rule::Judged { variables } => variables * 2,
            Rule::LaidOut { slots } => slots * 2,
            Rule::InstanceAt { variables, .. } => *variables,
        }
    }

    /// **The closed rule**: every output over closed `operands`, as the closed path runs it.
    /// `None` where the rule has no answer, as for a piece of a type with none.
    pub fn closed<'x>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'x>,
        operands: &[DeclaredType<KType>],
    ) -> Option<&'x [KType]> {
        match (*self, operands) {
            (Rule::Fill { filled }, _) => {
                return fill_closed(types, scratch, operands, filled as usize);
            }
            (
                Rule::Solved {
                    involved,
                    pinned,
                    exact,
                    ..
                },
                [DeclaredType::Scheme(scheme), rest @ ..],
            ) => return solved_closed(types, scratch, *scheme, rest, (involved, pinned, exact)),
            (
                Rule::InstanceAt { bounded, .. },
                [
                    DeclaredType::Scheme(scheme),
                    DeclaredType::Type(wanted),
                    bounds @ ..,
                ],
            ) => {
                let scheme = match bounded {
                    0 => *scheme,
                    _ => rebuilt_with(types, scratch, *scheme, bounded, bounds)?,
                };
                let record = instance_under(types, scratch, scheme, *wanted);
                let solved = record.solved.filter(|_| !record.reached.contains(&false))?;
                return Some(scratch.alloc_slice_copy(solved.types));
            }
            (Rule::Judged { .. }, [shape, ends @ ..]) => {
                let mut arguments = BumpVec::with_capacity_in(ends.len() / 2, scratch);
                arguments.extend(pairs(ends).map(|(lower, upper)| Interval {
                    lower: lower.into(),
                    upper: upper.into(),
                }));
                let shape = match shape {
                    DeclaredType::Type(shape) => DeclaredType::Type(Parametric::from(*shape)),
                    DeclaredType::Scheme(scheme) => DeclaredType::Scheme(*scheme),
                };
                let judged = judge_by_class(types, scratch, shape, &arguments);
                let mut outputs = BumpVec::new_in(scratch);
                match (judged.solve.intervals(), shape) {
                    (Some(intervals), _) => {
                        for interval in intervals {
                            outputs.push(types.concrete(interval.lower)?);
                            outputs.push(types.concrete(interval.upper)?);
                        }
                    }
                    (None, DeclaredType::Scheme(scheme)) => {
                        for bound in scheme.group(types).bounds {
                            outputs.extend([KType::NEVER, *bound]);
                        }
                    }
                    (None, DeclaredType::Type(_)) => return None,
                }
                return Some(outputs.leak());
            }
            _ => {}
        }
        let output = match (*self, operands) {
            (Rule::Piece(step), [DeclaredType::Type(base)]) => {
                types.concrete(types.projection(scratch, (*base).into(), step)?)?
            }
            (Rule::Meet, [DeclaredType::Type(a), DeclaredType::Type(b)]) => {
                meet(types, scratch, *a, *b)
            }
            (Rule::Retyped, [DeclaredType::Type(declared)]) => {
                types.concrete(retyped_to(types, scratch, (*declared).into()).lower)?
            }
            (Rule::Constructed, [DeclaredType::Type(head), DeclaredType::Type(payload)]) => {
                construction(types, scratch, *head, *payload).ok()?
            }
            (
                Rule::Construction { upper, projected },
                [
                    DeclaredType::Type(head),
                    DeclaredType::Type(payload),
                    owner @ ..,
                ],
            ) => {
                let built = construction(types, scratch, *head, *payload);
                if !upper {
                    return Some(scratch.alloc_slice_copy(&[built.unwrap_or(KType::NEVER)]));
                }
                let through = match (projected, owner) {
                    (true, [DeclaredType::Type(owner)]) => tag_union(types, scratch, *owner),
                    _ => None,
                };
                let built = built.map(Parametric::from);
                types.concrete(constructed_upper(types, scratch, (*head, through), built))?
            }
            (Rule::Member(name), [DeclaredType::Type(owner)]) => {
                declared_member(types, scratch, *owner, name)?
            }
            (Rule::Applied { sole }, [DeclaredType::Type(head), DeclaredType::Type(arguments)]) => {
                let declared = head_params(types, *head)?;
                let named = match sole {
                    false => fields(types, scratch, *arguments)?,
                    true => {
                        let [param] = declared else {
                            return None;
                        };
                        scratch
                            .alloc_slice_copy(&[(BinderSymbol::Type(*param), (*arguments).into())])
                    }
                };
                types.concrete(applied(types, scratch, *head, declared, named)?)?
            }
            (Rule::Pinned, [DeclaredType::Type(signature), DeclaredType::Type(pins)]) => {
                let pins = fields(types, scratch, *pins)?;
                types.concrete(pinned(types, scratch, *signature, pins)?)?
            }
            (Rule::Variant(label), [DeclaredType::Type(union)]) => {
                closed_variant(types, *union, label)?
            }
            (Rule::Payload(label), [DeclaredType::Type(union)]) => {
                let variant = closed_variant(types, *union, label)?;
                representation(types, scratch, variant).unwrap_or(KType::ANY)
            }
            (Rule::Uncovered, [DeclaredType::Type(union), named @ ..]) => {
                let mut payloads = BumpVec::new_in(scratch);
                for variant in variants(types, *union)? {
                    let variant = types.concrete(variant)?;
                    if named.contains(&DeclaredType::Type(variant)) {
                        continue;
                    }
                    match representation(types, scratch, variant) {
                        Some(payload) => payloads.push(payload),
                        None => return Some(scratch.alloc_slice_copy(&[KType::NEVER, KType::ANY])),
                    }
                }
                let bound = under(types.union_of(scratch, &payloads).into());
                let ends = [types.concrete(bound.lower)?, types.concrete(bound.upper)?];
                return Some(scratch.alloc_slice_copy(&ends));
            }
            (Rule::FieldAbove(name), [DeclaredType::Type(ty)]) => {
                field_above(types, scratch, *ty, name)
            }
            (Rule::FieldUnder(name), [DeclaredType::Type(ty)]) => match types.node(*ty) {
                TypeNode::Record { fields } => fields.get(name.symbol()).unwrap_or(KType::NEVER),
                _ => KType::NEVER,
            },
            (Rule::SignatureValue(name), [DeclaredType::Type(signature)]) => {
                signature_value(types, scratch, *signature, name)?
            }
            (Rule::Lacks(name), [DeclaredType::Type(ty)]) => match types.node(*ty) {
                TypeNode::Record { fields } if fields.get(name.symbol()).is_none() => *ty,
                _ if signature_lacks(types, (*ty).into(), name) => *ty,
                _ => return None,
            },
            (Rule::Fielded, [DeclaredType::Type(ty)]) => match types.node(*ty) {
                TypeNode::Record { .. }
                | TypeNode::Signature { .. }
                | TypeNode::SignatureApply { .. }
                | TypeNode::SignatureMeet { .. } => *ty,
                _ => return None,
            },
            (
                Rule::Application,
                [
                    DeclaredType::Type(head_lower),
                    DeclaredType::Type(head_upper),
                    DeclaredType::Type(lower),
                    DeclaredType::Type(upper),
                    DeclaredType::Type(result),
                ],
            ) => {
                let head = Interval {
                    lower: *head_lower,
                    upper: *head_upper,
                };
                let payload = Interval {
                    lower: *lower,
                    upper: *upper,
                };
                let applied = applied_closed(types, scratch, *result, head, payload)?;
                return Some(scratch.alloc_slice_copy(&[applied.lower, applied.upper]));
            }
            (
                Rule::MemberApplication(name),
                [
                    DeclaredType::Type(member_lower),
                    DeclaredType::Type(carried),
                    DeclaredType::Type(lower),
                    DeclaredType::Type(upper),
                    DeclaredType::Type(result),
                ],
            ) => {
                let payload = Interval {
                    lower: *lower,
                    upper: *upper,
                };
                let member = (*member_lower, *carried, name);
                let applied = member_applied_closed(types, scratch, *result, member, payload)?;
                return Some(scratch.alloc_slice_copy(&[applied.lower, applied.upper]));
            }
            (Rule::MemberInstance { name, .. }, [DeclaredType::Type(carried), wanted @ ..]) => {
                let wanted = match wanted {
                    [] => None,
                    [DeclaredType::Type(wanted)] => Some(*wanted),
                    _ => return None,
                };
                match member_instance_closed(types, scratch, *carried, name, wanted) {
                    MemberInstance::Plain(ktype) | MemberInstance::Instance { ktype, .. } => ktype,
                    MemberInstance::Undeclared
                    | MemberInstance::Unfixed { .. }
                    | MemberInstance::NoInstance(_) => return None,
                }
            }
            (
                Rule::LaidOut { .. },
                [
                    DeclaredType::Type(lower),
                    DeclaredType::Type(upper),
                    DeclaredType::Type(names),
                ],
            ) => {
                let mut named = BumpVec::new_in(scratch);
                named.extend(
                    fields(types, scratch, *names)?
                        .iter()
                        .map(|(name, _)| *name),
                );
                let payload = Interval {
                    lower: *lower,
                    upper: *upper,
                };
                let laid = laid_out_closed(types, scratch, &named, payload)?;
                let mut outputs = BumpVec::with_capacity_in(laid.len() * 2, scratch);
                outputs.extend(laid.iter().flat_map(|each| [each.lower, each.upper]));
                return Some(outputs.leak());
            }
            (Rule::Wanted, [DeclaredType::Type(head)]) => {
                wanted_closed(types, scratch, *head).unwrap_or(KType::ANY)
            }
            (Rule::View, [DeclaredType::Type(shown), _]) => shown_view(types, scratch, *shown)?,
            (Rule::SliceBounds, [DeclaredType::Type(lower)]) => {
                (!bounds_refuse(types, scratch, *lower)).then_some(*lower)?
            }
            _ => return None,
        };
        Some(scratch.alloc_slice_copy(&[output]))
    }

    /// Each output the rule gives at every binding within `operands`' ends, written over names: a
    /// piece the bound guarantees as its projection, which substitutes to what the closed rule
    /// gives. `None` for an output no spelling over names gives.
    pub fn spelled<'x>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'x>,
        operands: &[DeclaredType<Parametric>],
    ) -> &'x [Option<Parametric>] {
        if let Rule::Fill { filled } = *self {
            return fill_spelled(types, scratch, operands, filled as usize);
        }
        let output = match (*self, operands) {
            (Rule::Piece(step), [DeclaredType::Type(base)]) => {
                types.projection(scratch, *base, step)
            }
            // A field a bound guarantees is the projection there, at every binding.
            (Rule::FieldAbove(name) | Rule::FieldUnder(name), [DeclaredType::Type(base)]) => {
                types.projection(scratch, *base, Step::Field(name))
            }
            // A type meets itself and `Any` at itself, at every binding.
            (Rule::Meet, [DeclaredType::Type(a), DeclaredType::Type(b)])
                if a == b || *b == KType::ANY.into() =>
            {
                Some(*a)
            }
            (Rule::Meet, [DeclaredType::Type(a), DeclaredType::Type(b)])
                if *a == KType::ANY.into() =>
            {
                Some(*b)
            }
            // A retype that is a point over names is one at every binding: its head decides it.
            (Rule::Retyped, [DeclaredType::Type(declared)]) => {
                let retyped = retyped_to(types, scratch, *declared);
                (retyped == Interval::point(*declared)).then_some(*declared)
            }
            _ => None,
        };
        scratch.alloc_slice_copy(&[output])
    }

    /// Closed ends holding what every binding gives output `output`, the ends of the derived name
    /// standing for it.
    pub fn ends(
        &self,
        (types, scratch): (&Chain<'_, '_, '_, '_>, BumpAllocator<'_>),
        operands: &[DeclaredType<Parametric>],
        output: u8,
    ) -> (KType, KType) {
        match (self, operands) {
            (Rule::Retyped, [DeclaredType::Type(declared)]) => {
                (KType::NEVER, bound_above(types, scratch, *declared))
            }
            // Every binding's field lies under the field above the bound.
            (Rule::FieldAbove(name) | Rule::FieldUnder(name), [DeclaredType::Type(ty)]) => {
                let bound = bound_above(types, scratch, *ty);
                (KType::NEVER, field_above(types, scratch, bound, *name))
            }
            // The fill only narrows a record, so every binding's lies under the uppers' bounds.
            (Rule::Fill { filled }, _) => {
                let mut bounds = BumpVec::new_in(scratch);
                for (_, upper) in pairs(operands).take(*filled as usize) {
                    bounds.push(bound_above(types, scratch, upper));
                }
                (KType::NEVER, types.union_of(scratch, &bounds))
            }
            // A variable's solution lies under its bound.
            (Rule::Solved { .. }, [DeclaredType::Scheme(scheme), ..]) => {
                let bounds = scheme.group(types).bounds;
                let bound = bounds.get(usize::from(output)).copied();
                (KType::NEVER, bound.unwrap_or(KType::ANY))
            }
            // A closed family's every construction lies under the bare family, or the union its
            // owner peels to.
            (
                Rule::Construction {
                    upper: true,
                    projected,
                },
                [DeclaredType::Type(head), _, owner @ ..],
            ) => {
                let head = types.concrete(*head);
                let through = match (projected, owner) {
                    (true, [DeclaredType::Type(owner)]) => types
                        .concrete(*owner)
                        .and_then(|owner| tag_union(types, scratch, owner)),
                    _ => None,
                };
                let constructible = head.and_then(|head| types.member_schema(head));
                match (head, constructible) {
                    (
                        Some(head),
                        Some(
                            NodeSchema::NewType(_)
                            | NodeSchema::TypeConstructor {
                                representation: Some(_),
                                ..
                            },
                        ),
                    ) => (KType::NEVER, through.unwrap_or(head)),
                    _ => (KType::NEVER, KType::ANY),
                }
            }
            // Every binding's module lies under the operand's bound, so declares the member at most
            // where the bound does; `Any` where the bound declares none.
            (Rule::SignatureValue(name), [DeclaredType::Type(ty)]) => {
                let bound = bound_above(types, scratch, *ty);
                let read = signature_value(types, scratch, bound, *name);
                (KType::NEVER, read.unwrap_or(KType::ANY))
            }
            // A view lies under the application it is ascribed to, as `:|` admits it.
            (Rule::View, [_, DeclaredType::Type(application)]) => {
                (KType::NEVER, bound_above(types, scratch, *application))
            }
            (Rule::Judged { .. }, [DeclaredType::Scheme(scheme), ..]) => {
                let bounds = scheme.group(types).bounds;
                let bound = bounds.get(usize::from(output / 2)).copied();
                (KType::NEVER, bound.unwrap_or(KType::ANY))
            }
            _ => (KType::NEVER, KType::ANY),
        }
    }

    /// Write the spelling output `output` renders as, which names the derived name standing for
    /// it: a piece as its projection, `(ELEMENT OF Elt)`, a meet as `(a & b)`. Parenthesized and
    /// holding a space, so it is a type spelling no token is.
    pub fn spell(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        (types, symbols): (&Chain<'_, '_, '_, '_>, &SymbolInterner),
        operands: &[DeclaredType<Parametric>],
        output: u8,
    ) -> std::fmt::Result {
        let shown =
            |f: &mut std::fmt::Formatter<'_>, operand: DeclaredType<Parametric>| match operand {
                DeclaredType::Type(ty) => write!(f, "{}", display_name(ty, types, symbols)),
                DeclaredType::Scheme(_) => f.write_str("Any"),
            };
        match (*self, operands) {
            (Rule::Piece(step), [DeclaredType::Type(base)]) => {
                write!(f, "{}", display_projection(*base, step, types, symbols))
            }
            (Rule::Retyped, [declared]) => {
                f.write_str("(RETYPED TO ")?;
                shown(f, *declared)?;
                f.write_str(")")
            }
            (Rule::InstanceAt { .. }, [_, wanted, ..]) => {
                write!(f, "(INSTANCE #{output} AT ")?;
                shown(f, *wanted)?;
                f.write_str(")")
            }
            (Rule::Constructed, [head, payload]) => {
                f.write_str("(")?;
                shown(f, *head)?;
                f.write_str(" CONSTRUCTED FROM ")?;
                shown(f, *payload)?;
                f.write_str(")")
            }
            (Rule::Member(name), [owner]) => {
                write!(f, "(MEMBER {} OF ", symbols.display(name))?;
                shown(f, *owner)?;
                f.write_str(")")
            }
            (Rule::Applied { sole: true }, [head, argument]) => {
                f.write_str("(")?;
                shown(f, *argument)?;
                f.write_str(" -> ")?;
                shown(f, *head)?;
                f.write_str(")")
            }
            (Rule::Applied { sole: false }, [head, DeclaredType::Type(arguments)]) => {
                f.write_str("(")?;
                shown(f, *head)?;
                f.write_str(" ")?;
                written(f, (types, symbols), *arguments)?;
                f.write_str(")")
            }
            (Rule::Pinned, [signature, DeclaredType::Type(pins)]) => {
                f.write_str("(")?;
                shown(f, *signature)?;
                f.write_str(" WITH ")?;
                written(f, (types, symbols), *pins)?;
                f.write_str(")")
            }
            (Rule::Variant(label) | Rule::Payload(label), [union]) => {
                let read = match self {
                    Rule::Variant(_) => "VARIANT",
                    _ => "PAYLOAD",
                };
                write!(f, "({read} {} OF ", symbols.display(label))?;
                shown(f, *union)?;
                f.write_str(")")
            }
            (Rule::Uncovered, [union, named @ ..]) => {
                let read = match output {
                    0 => "(LOWER UNCOVERED BY",
                    _ => "(UPPER UNCOVERED BY",
                };
                f.write_str(read)?;
                for variant in named {
                    f.write_str(" ")?;
                    shown(f, *variant)?;
                }
                f.write_str(" OF ")?;
                shown(f, *union)?;
                f.write_str(")")
            }
            (Rule::FieldAbove(name) | Rule::FieldUnder(name), [ty]) => {
                let read = match self {
                    Rule::FieldAbove(_) => "ABOVE",
                    _ => "UNDER",
                };
                write!(f, "(FIELD {} {read} ", symbols.display(name.symbol()))?;
                shown(f, *ty)?;
                f.write_str(")")
            }
            (Rule::Fielded, [ty]) => {
                f.write_str("(FIELDED ")?;
                shown(f, *ty)?;
                f.write_str(")")
            }
            (Rule::Lacks(name), [ty]) => {
                write!(f, "(LACKS {} OF ", symbols.display(name.symbol()))?;
                shown(f, *ty)?;
                f.write_str(")")
            }
            (Rule::SignatureValue(name), [signature]) => {
                write!(f, "(VALUE {} OF ", symbols.display(name.symbol()))?;
                shown(f, *signature)?;
                f.write_str(")")
            }
            (Rule::MemberApplication(name), [_, carried, _, argument, _]) => {
                f.write_str(match output {
                    0 => "(LOWER ",
                    _ => "(UPPER ",
                })?;
                write!(f, "(VALUE {} OF ", symbols.display(name.symbol()))?;
                shown(f, *carried)?;
                f.write_str(") APPLIED TO ")?;
                shown(f, *argument)?;
                f.write_str(")")
            }
            (Rule::MemberInstance { name, .. }, [carried, wanted @ ..]) => {
                write!(
                    f,
                    "(INSTANCE OF (VALUE {} OF ",
                    symbols.display(name.symbol())
                )?;
                shown(f, *carried)?;
                f.write_str(")")?;
                if let [wanted] = wanted {
                    f.write_str(" UNDER ")?;
                    shown(f, *wanted)?;
                }
                f.write_str(")")
            }
            (Rule::Application, [_, head, _, argument, _]) => {
                f.write_str(match output {
                    0 => "(LOWER ",
                    _ => "(UPPER ",
                })?;
                shown(f, *head)?;
                f.write_str(" APPLIED TO ")?;
                shown(f, *argument)?;
                f.write_str(")")
            }
            (Rule::LaidOut { .. }, [_, upper, _]) => {
                let end = match output % 2 {
                    0 => "LOWER",
                    _ => "UPPER",
                };
                write!(f, "({end} SLOT #{} OF ", output / 2)?;
                shown(f, *upper)?;
                f.write_str(")")
            }
            (Rule::SliceBounds, [lower]) => {
                f.write_str("(BOUNDS OF ")?;
                shown(f, *lower)?;
                f.write_str(")")
            }
            (Rule::Wanted, [head]) => {
                f.write_str("(WANTED BY ")?;
                shown(f, *head)?;
                f.write_str(")")
            }
            (Rule::View, [application, _]) => {
                f.write_str("(VIEW OF ")?;
                shown(f, *application)?;
                f.write_str(")")
            }
            (Rule::Construction { upper, .. }, [head, payload, ..]) => {
                f.write_str(if upper { "(UPPER " } else { "(LOWER " })?;
                shown(f, *head)?;
                f.write_str(" BUILT FROM ")?;
                shown(f, *payload)?;
                f.write_str(")")
            }
            (Rule::Meet, [a, b]) => {
                f.write_str("(")?;
                shown(f, *a)?;
                f.write_str(" & ")?;
                shown(f, *b)?;
                f.write_str(")")
            }
            (Rule::Fill { filled }, _) => {
                f.write_str(match output {
                    0 => "(LOWER FILL OF ",
                    _ => "(UPPER FILL OF ",
                })?;
                for (at, (lower, upper)) in pairs(operands).enumerate() {
                    let gate = at >= filled as usize;
                    f.write_str(match (at, gate) {
                        (0, _) => "",
                        (_, false) => ", ",
                        (_, true) => " BY ",
                    })?;
                    shown(f, lower.into())?;
                    f.write_str(" .. ")?;
                    shown(f, upper.into())?;
                }
                f.write_str(")")
            }
            (Rule::Solved { .. }, [_, contributions @ ..]) => {
                write!(f, "(SOLVED #{output} FROM ")?;
                for (at, contribution) in contributions.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    shown(f, *contribution)?;
                }
                f.write_str(")")
            }
            (Rule::Judged { .. }, [_, ends @ ..]) => {
                let end = match output % 2 {
                    0 => "LOWER",
                    _ => "UPPER",
                };
                write!(f, "({end} #{} JUDGED FROM ", output / 2)?;
                for (at, (lower, upper)) in pairs(ends).enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    shown(f, lower.into())?;
                    f.write_str(" .. ")?;
                    shown(f, upper.into())?;
                }
                f.write_str(")")
            }
            _ => unreachable!("a rule's operands are those it reads"),
        }
    }
}

/// [`Rule::FieldAbove`]'s closed function: the field `name` of a record, joined over a union's
/// members, `Never` at `Never`, and `Any` where a value under `ty` may hold no such field.
fn field_above(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: KType,
    name: BinderSymbol,
) -> KType {
    match types.node(ty) {
        TypeNode::Never => KType::NEVER,
        TypeNode::Record { fields } => fields.get(name.symbol()).unwrap_or(KType::ANY),
        TypeNode::Union { members } => {
            let mut fields = BumpVec::with_capacity_in(members.len(), scratch);
            fields.extend(
                members
                    .iter()
                    .map(|member| field_above(types, scratch, member, name)),
            );
            types.union_of(scratch, &fields)
        }
        _ => KType::ANY,
    }
}

/// Write the record type `record` a spelling's rule reads as it is written: its fields as
/// `{Name = Type, …}`.
fn written(
    f: &mut std::fmt::Formatter<'_>,
    (types, symbols): (&Chain<'_, '_, '_, '_>, &SymbolInterner),
    record: Parametric,
) -> std::fmt::Result {
    let TypeNode::Record { fields } = types.node(record) else {
        return write!(f, "{}", display_name(record, types, symbols));
    };
    f.write_str("{")?;
    for (at, (name, ty)) in fields.iter().enumerate() {
        let between = if at == 0 { "" } else { ", " };
        write!(
            f,
            "{between}{} = {}",
            symbols.display(name.symbol()),
            display_name(ty, types, symbols)
        )?;
    }
    f.write_str("}")
}

/// The fields of the record type `record`, a rule's arguments or names by name: `None` where it is
/// no record.
fn fields<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    record: KType,
) -> Option<&'x [(BinderSymbol, Parametric)]> {
    let TypeNode::Record { fields } = types.node(record) else {
        return None;
    };
    let mut named = BumpVec::with_capacity_in(fields.len(), scratch);
    named.extend(fields.iter().map(|(name, ty)| (name, Parametric::from(ty))));
    Some(named.leak())
}

/// [`Rule::Solved`]'s closed function: the class step over the scheme's `involved` slots at the
/// `operands` they contribute, pinned as the operands past them pin, each variable read from above
/// through the solution. `None` where the step admits no solve.
fn solved_closed<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    scheme: Scheme,
    operands: &[DeclaredType<KType>],
    (involved, pinned, exact): (u64, u64, bool),
) -> Option<&'x [KType]> {
    let bit = |mask: u64, at: usize| at < 64 && mask & 1 << at != 0;
    let bounds = scheme.group(types).bounds;
    let mut operands = operands.iter().map(|operand| match operand {
        DeclaredType::Type(ty) => Some(Parametric::from(*ty)),
        DeclaredType::Scheme(_) => None,
    });
    let mut slots = BumpVec::new_in(scratch);
    for (at, slot) in scheme_slots(scheme, types).enumerate() {
        if bit(involved, at) {
            slots.push((slot, operands.next()??));
        }
    }
    let mut pins = BumpVec::with_capacity_in(bounds.len(), scratch);
    for variable in 0..bounds.len() {
        pins.push(match bit(pinned, variable) {
            true => Some(operands.next()??),
            false => None,
        });
    }
    let record = class_step(types, scratch, bounds, &pins, &slots, exact);
    let solved = record.solved?;
    let mut outputs = BumpVec::with_capacity_in(bounds.len(), scratch);
    for interval in solved.intervals.iter() {
        outputs.push(types.concrete(interval.upper)?);
    }
    Some(outputs.leak())
}

/// `operands`, a fill's, as pairs of ends: each element's `[lower, upper]`.
fn pairs<H: Copy>(operands: &[DeclaredType<H>]) -> impl Iterator<Item = (H, H)> + '_ {
    operands.chunks_exact(2).map(|pair| match pair {
        [DeclaredType::Type(lower), DeclaredType::Type(upper)] => (*lower, *upper),
        _ => unreachable!("a fill's operands are types"),
    })
}

/// [`Rule::Fill`]'s closed function: the run's fill where every pair is exact, else the ends.
fn fill_closed<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    operands: &[DeclaredType<KType>],
    filled: usize,
) -> Option<&'x [KType]> {
    if pairs(operands).all(|(lower, upper)| lower == upper) {
        let points = pairs(operands).take(filled).map(|(_, upper)| upper);
        let fill = filled_union(types, scratch, points);
        return Some(scratch.alloc_slice_copy(&[fill, fill]));
    }
    let (mut lowers, mut uppers) = (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
    for (lower, upper) in pairs(operands).take(filled) {
        lowers.push(match may_hold_record(types, scratch, lower.into()) {
            true => KType::NEVER,
            false => lower,
        });
        uppers.push(upper);
    }
    let ends = [
        types.union_of(scratch, &lowers),
        types.union_of(scratch, &uppers),
    ];
    Some(scratch.alloc_slice_copy(&ends))
}

/// [`Rule::Fill`] written over names: where no end of a filled pair may hold a record, the fill
/// is the union and nothing is read as `Never`, so the ends are the unions of the lower and upper
/// ends at every binding, exact or not. Where some pair is inexact at every binding — two closed
/// ends that differ — the upper end is the uppers' union. Nothing else is written.
fn fill_spelled<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    operands: &[DeclaredType<Parametric>],
    filled: usize,
) -> &'x [Option<Parametric>] {
    let holds = |ty: Parametric| may_hold_record(types, scratch, ty);
    let (mut lowers, mut uppers) = (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
    let mut plain = true;
    for (lower, upper) in pairs(operands).take(filled) {
        plain &= !holds(lower) && !holds(upper);
        lowers.push(lower);
        uppers.push(upper);
    }
    let (lower, upper) = (
        types.union_of(scratch, &lowers),
        types.union_of(scratch, &uppers),
    );
    let inexact = pairs(operands).any(|(lower, upper)| {
        let closed = |ty| types.concrete(ty);
        closed(lower)
            .zip(closed(upper))
            .is_some_and(|(a, b)| a != b)
    });
    let spelled = match (plain, inexact) {
        (true, _) => [Some(lower), Some(upper)],
        (false, true) => [None, Some(upper)],
        (false, false) => [None, None],
    };
    scratch.alloc_slice_copy(&spelled)
}

#[cfg(test)]
mod tests;

/// The closed `scheme` with the bound of each group position `bounded` names (a bit each) replaced
/// by the closed type beside it in `bounds`, in order ([`rebuilt_scheme`]): `None` where a bound
/// is no type or the rebuilt scheme is not one.
fn rebuilt_with(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
    bounded: u64,
    bounds: &[DeclaredType<KType>],
) -> Option<Scheme> {
    let positions = (0..64).filter(|at| bounded & (1 << at) != 0);
    let mut written = BumpVec::with_capacity_in(bounds.len(), scratch);
    for (at, bound) in positions.zip(bounds) {
        written.push((at, bound.as_type()?));
    }
    rebuilt_scheme(types, scratch, scheme.into(), &written)?.as_scheme()
}
