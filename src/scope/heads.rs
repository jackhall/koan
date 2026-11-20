//! **Spellings over a head**: a type spelled by reading a member off an owner (`Shape.Red`), or by
//! applying a head — a family at its arguments, a signature `WITH` its pins, a code kind `NEEDING`
//! names or `UNBUILT` — and a construction's upper end over its head. Each is a closed function of
//! the head here, which the elaborator runs over a
//! closed head, and which the [rule](super::Rule) of the derived name it lays over a run-bound head
//! runs under a binding, so a binding written in spells what a fresh load spells. Each is `None`
//! where the head admits no such spelling, the refusal the elaborator words. The signature an
//! opaque view carries over the application it shows ([`shown_view`]) is here too: the view's
//! rule, and the knot's view, build it through one door. So is a value member a closed signature
//! declares ([`signature_value`]), which a read through a `:Module` binder's carried type reads.

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, Symbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, Interval, Judged, KType, Members, NodeSchema, Parametric, SchemaDraft, SigOrigin,
    SigSchema, SolveRecord, TypeNode, Verdict, constructor_param_names, judge_by_class, member,
    scheme_slots, shape_slots, substitute_parameters,
};
use crate::values::{Chain, ConstructionRefused};

/// The type the record under `owner` declares `name` with, read through every newtype layer above
/// it — a `NEWTYPE`'s representation, a union variant's payload. `None` when no record lies under
/// `owner` or it declares no `name`; a ring of newtypes with no record under it, `NEWTYPE Loop =
/// Loop`, is peeled once round and then refused.
pub fn declared_field(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    owner: KType,
    name: Symbol,
) -> Option<KType> {
    through_layers(types, scratch, owner, |layer| match types.node(layer) {
        TypeNode::Record { fields } => Some(fields.get(name)),
        _ => None,
    })
    .flatten()
}

/// The union a tag projection off `owner` reads its tags from: `owner` where it is a union, else
/// the first union its newtype layers peel down to, as [`declared_field`] peels to a record.
pub fn tag_union(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    owner: KType,
) -> Option<KType> {
    through_layers(types, scratch, owner, |layer| {
        types.is_union(layer).then_some(layer)
    })
}

/// The variant `name` a tag projection off `owner` names: the member `name` of its
/// [`tag_union`]. `None` where no union lies under `owner` or it declares no `name`.
pub fn declared_variant(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    owner: KType,
    name: Symbol,
) -> Option<KType> {
    tag_union(types, scratch, owner).and_then(|union| types.union_member_named(union, name))
}

/// The member `name` a type spelled `owner.name` reads: a variant of the union under `owner`, else
/// a field of the record under it.
pub fn declared_member(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    owner: KType,
    name: Symbol,
) -> Option<KType> {
    declared_variant(types, scratch, owner, name)
        .or_else(|| declared_field(types, scratch, owner, name))
}

/// The first of `owner` and the representations its newtype layers peel down to — a `NEWTYPE`'s,
/// a union variant's payload — that `read` answers, stopping at a layer that is no newtype or one
/// already peeled.
fn through_layers<R>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    owner: KType,
    read: impl Fn(KType) -> Option<R>,
) -> Option<R> {
    let mut peeled = BumpVec::new_in(scratch);
    let mut layer = owner;
    loop {
        if let Some(answer) = read(layer) {
            return Some(answer);
        }
        match types.member_schema(layer) {
            Some(NodeSchema::NewType(repr)) if !peeled.contains(&repr) => {
                peeled.push(layer);
                layer = repr;
            }
            _ => return None,
        }
    }
}

/// The parameterized union `union`, not yet applied, applied at `arguments`: each of its variants
/// applied at the same arguments, unioned. The caller has checked `arguments` against the
/// parameters the variants declare.
pub fn applied_union(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    union: KType,
    arguments: &[(BinderSymbol, Parametric)],
) -> Parametric {
    let TypeNode::Union { members } = types.node(union) else {
        unreachable!("`applied_union` applies a union")
    };
    debug_assert!(
        (members.iter())
            .all(|member| !matches!(types.node(member), TypeNode::ConstructorApply { .. })),
        "`applied_union` applies an unapplied union"
    );
    let mut applied = BumpVec::with_capacity_in(members.len(), scratch);
    applied.extend(
        members
            .iter()
            .map(|member| types.constructor_apply(scratch, member, arguments)),
    );
    types.union_of(scratch, &applied)
}

/// A union's variants, as a `MATCH … UNDER` clause names them: `union`'s members in stored order,
/// or `None` where `union` is no `Union` node. A label names the member of its name: a declared
/// variant by its own name, and a variant applied to arguments, as `:(Number -> Option)` applies
/// each, by its constructor's. A structural member, `Number` in `:(Maybe | Number)`, has no name,
/// so no label names it.
pub fn variants<'tier>(
    types: &Chain<'tier, '_, '_, '_>,
    union: KType,
) -> Option<impl Iterator<Item = KType> + 'tier> {
    match types.node(union) {
        TypeNode::Union { members } => Some(members.iter()),
        _ => None,
    }
}

/// The member of `union` that `label` names, or `None` where none does or `union` is no union.
pub fn variant_named(types: &Chain<'_, '_, '_, '_>, union: KType, label: Symbol) -> Option<KType> {
    variants(types, union)?.find(|member| variant_label(types, *member) == Some(label))
}

/// The name a label gives `variant`: a declared variant's own, an applied one's constructor's, and
/// `None` for a structural member.
pub fn variant_label(types: &Chain<'_, '_, '_, '_>, variant: KType) -> Option<Symbol> {
    let declared = match types.node(variant) {
        TypeNode::ConstructorApply { constructor, .. } => constructor,
        _ => variant,
    };
    match types.node(declared) {
        TypeNode::SetMember { name, .. } => Some(name.symbol()),
        _ => None,
    }
}

/// The parameter names a declared constructor head takes: a family's, or — for a union every
/// member of which takes one parameter set — that set, since applying the union applies each
/// member.
pub fn head_params<'n>(
    types: &Chain<'n, '_, '_, '_>,
    constructor: KType,
) -> Option<&'n [TypeSymbol]> {
    if let TypeNode::Union { members } = types.node(constructor) {
        let mut members = members.iter();
        let names = constructor_param_names(members.next()?, types)?;
        return members
            .all(|member| constructor_param_names(member, types) == Some(names))
            .then_some(names);
    }
    constructor_param_names(constructor, types)
}

/// `constructor` applied to `arguments`, keyed by `declared`, the parameter names it takes: every
/// parameter named once, and no name it does not declare. A parameterized union applies each of
/// its variants at the same arguments. `None` where the names do not match.
pub fn applied(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    constructor: KType,
    declared: &[TypeSymbol],
    arguments: &[(BinderSymbol, Parametric)],
) -> Option<Parametric> {
    let named =
        |name: &BinderSymbol| matches!(name, BinderSymbol::Type(name) if declared.contains(name));
    let repeated = (arguments.iter().enumerate())
        .any(|(index, (name, _))| arguments[..index].iter().any(|(seen, _)| seen == name));
    if arguments.len() != declared.len()
        || !arguments.iter().all(|(name, _)| named(name))
        || repeated
    {
        return None;
    }
    if types.is_union(constructor) {
        return Some(applied_union(types, scratch, constructor, arguments));
    }
    Some(types.constructor_apply(scratch, constructor, arguments))
}

/// `signature` with `pins` fixing head parameters by name: `None` where it is no declared
/// signature or a pin names none of its parameters.
pub fn pinned(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    signature: KType,
    pins: &[(BinderSymbol, Parametric)],
) -> Option<Parametric> {
    let TypeNode::Signature { schema, .. } = types.node(signature) else {
        return None;
    };
    if schema.origin != SigOrigin::Declared {
        return None;
    }
    for (name, _) in pins {
        let BinderSymbol::Type(parameter) = name else {
            return None;
        };
        member(schema.parameters, *parameter)?;
    }
    Some(types.signature_apply(scratch, signature, pins))
}

/// The code kind `kind` needing `names`: `None` where `kind` is no code kind below `Code`.
pub fn needing(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    kind: KType,
    names: &[BinderSymbol],
) -> Option<KType> {
    kind.code_parent()?;
    Some(types.code_needing(scratch, kind, names))
}

/// `UNBUILT kind`: `None` where `kind` is neither `Code` nor a code kind below it.
pub fn unbuilt(types: &Chain<'_, '_, '_, '_>, kind: KType) -> Option<KType> {
    if kind != KType::ANY_CODE && kind.code_parent().is_none() {
        return None;
    }
    Some(types.code_unbuilt(kind))
}

/// The upper end of a construction of the closed `head`, from `built`, what the construction gives:
/// the constructed type, or through `through` — the union a tag projection head reaches its variant
/// through — the union applied at the arguments the payload solves; `Any` where `head` constructs
/// nothing; and the bare head, or the union, where the payload fits or solves no instance.
pub fn constructed_upper<T: Copy + Into<Parametric>>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    (head, through): (KType, Option<KType>),
    built: Result<Parametric, ConstructionRefused<T>>,
) -> Parametric {
    match (built, through) {
        (Err(ConstructionRefused::NotConstructible(_)), _) => KType::ANY.into(),
        (Ok(constructed), Some(union)) => match types.node(constructed) {
            TypeNode::ConstructorApply { arguments, .. } => {
                let mut solved = BumpVec::with_capacity_in(arguments.len(), scratch);
                solved.extend(arguments.iter());
                applied_union(types, scratch, union, &solved)
            }
            _ => union.into(),
        },
        (Ok(constructed), None) => constructed,
        (Err(_), _) => through.unwrap_or(head).into(),
    }
}

/// The signature the view itself carries: each of `sig`'s parameters a manifest member at what `to`
/// gives it, each manifest member, value slot and keyworded member at its declared type read under
/// `to`. A view's signature is a module's, so it has no parameters, and it fits the signature it
/// was ascribed to.
pub fn view_signature(
    sig: &SigSchema<'_>,
    to: Members<'_, TypeSymbol, KType>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    let mut draft = SchemaDraft::new(scratch);
    for (name, _) in sig.parameters.iter().copied() {
        draft.insert_manifest(name, member(to, name).expect("every parameter is bound"));
    }
    for (name, fixed) in sig.manifest_members.iter().copied() {
        draft.insert_manifest(name, substitute_parameters(types, scratch, fixed, to));
    }
    for (name, declared) in sig.value_slots.iter().copied() {
        draft.insert_value_slot(name, substitute_parameters(types, scratch, declared, to));
    }
    for declared in sig.keyworded.iter().copied() {
        draft.push_keyworded(substitute_parameters(types, scratch, declared, to));
    }
    types.signature(scratch, draft)
}

/// The type every module under the closed `signature` declares its value member `name` at: a
/// signature's or an application's under its pins, a union's the join of its members', a meet's
/// the meet of what each member declaring it declares, and `Never`'s `Never`, since no module
/// reaches the read. `None` where it declares no such member, declares it at a scheme, or leaves a
/// head parameter unpinned.
pub fn signature_value(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    signature: KType,
    name: ValueSymbol,
) -> Option<KType> {
    if signature == KType::NEVER {
        return Some(KType::NEVER);
    }
    match types.node(signature) {
        TypeNode::Union { members } => {
            let mut joined = BumpVec::with_capacity_in(members.len(), scratch);
            for each in members.iter() {
                joined.push(signature_value(types, scratch, each, name)?);
            }
            Some(types.union_of(scratch, &joined))
        }
        TypeNode::SignatureMeet { members } => {
            let mut met: Option<KType> = None;
            for each in members.iter() {
                let read = match declared_value(types, scratch, each, name) {
                    Declared::No => continue,
                    Declared::Unread => return None,
                    Declared::At(read) => read,
                };
                met = Some(met.map_or(read, |held| {
                    crate::type_lattice::meet(types, scratch, held, read)
                }));
            }
            met
        }
        _ => match declared_value(types, scratch, signature, name) {
            Declared::At(read) => Some(read),
            Declared::No | Declared::Unread => None,
        },
    }
}

/// What a signature or an application of one declares a value member at.
enum Declared {
    /// No such member.
    No,
    /// A scheme, or a type over a head parameter the application leaves unpinned.
    Unread,
    /// This type.
    At(KType),
}

/// What the closed `signature`, a signature or an application of one, declares its value member
/// `name` at, under its pins.
fn declared_value(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    signature: KType,
    name: ValueSymbol,
) -> Declared {
    let (declared, pins) = match types.node(signature) {
        TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
        _ => (signature, None),
    };
    let TypeNode::Signature { schema, .. } = types.node(declared) else {
        return Declared::No;
    };
    let Some((_, read)) = schema.value_slots.iter().find(|(held, _)| *held == name) else {
        return Declared::No;
    };
    // A parameter the application leaves unpinned stays in the read, which is then not closed.
    let mut to = BumpVec::new_in(scratch);
    for (parameter, _) in schema.parameters.iter() {
        if let Some(pin) = pins.and_then(|pins| pins.get(BinderSymbol::Type(*parameter).symbol())) {
            to.push((*parameter, pin));
        }
    }
    let read = substitute_parameters(types, scratch, *read, Members::from_pairs(scratch, to));
    match read.as_type().and_then(|read| types.concrete(read)) {
        Some(read) => Declared::At(read),
        None => Declared::Unread,
    }
}

/// Whether every type at or above `lower` lies outside the closed `asked`, read off names no pin
/// changes: `lower` a signature, an application of one or a meet of those, and `asked` a record,
/// which no signature type fits, or a signature naming a value slot or a manifest member none of
/// `lower`'s schemas declares, which *fits* asks for by name. `false` for every other head, a
/// variable included, whose binding decides it.
pub fn signature_outside(types: &Chain<'_, '_, '_, '_>, lower: Parametric, asked: KType) -> bool {
    let lacks = |declares: &dyn Fn(&SigSchema<'_>) -> bool| {
        offered_declares(types, lower, declares) == Some(false)
    };
    match types.node(asked) {
        TypeNode::Record { .. } => offered_declares(types, lower, &|_| true).is_some(),
        TypeNode::Signature { schema: asked, .. } => {
            (asked.value_slots.iter())
                .any(|(name, _)| lacks(&|schema| member(schema.value_slots, *name).is_some()))
                || (asked.manifest_members.iter()).any(|(name, _)| {
                    lacks(&|schema| member(schema.manifest_members, *name).is_some())
                })
        }
        _ => false,
    }
}

/// [`judge_by_class`] of `shape` over `arguments`, judged *never* also where some argument's lower
/// end lies outside its closed slot by the names a signature declares ([`signature_outside`]),
/// which no binding changes. Every load-time verdict over arguments reads it.
pub fn judge_signed<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    shape: DeclaredType<Parametric>,
    arguments: &[Interval],
) -> Judged<'x> {
    let judged = judge_by_class(types, scratch, shape, arguments);
    let mut slots = BumpVec::new_in(scratch);
    match shape {
        DeclaredType::Type(shape) => slots.extend(shape_slots(shape, types)),
        DeclaredType::Scheme(scheme) => slots.extend(scheme_slots(scheme, types)),
    }
    let outside = judged.verdict != Verdict::Never
        && slots.len() == arguments.len()
        && (slots.iter().zip(arguments)).any(|(slot, argument)| {
            types
                .concrete(*slot)
                .is_some_and(|slot| signature_outside(types, argument.lower, slot))
        });
    match outside {
        true => Judged {
            verdict: Verdict::Never,
            solve: SolveRecord {
                solved: None,
                reached: &[],
                reproducible: true,
            },
        },
        false => judged,
    }
}

/// Whether some schema `ty` is built from declares a member, by `declares`: `ty` a signature, an
/// application of one or a meet of those. `None` for any other head.
fn offered_declares(
    types: &Chain<'_, '_, '_, '_>,
    ty: Parametric,
    declares: &dyn Fn(&SigSchema<'_>) -> bool,
) -> Option<bool> {
    match types.node(ty) {
        TypeNode::Signature { schema, .. } => Some(declares(&schema)),
        TypeNode::SignatureApply { signature, .. } => {
            offered_declares(types, signature.into(), declares)
        }
        TypeNode::SignatureMeet { members } => members.iter().try_fold(false, |any, member| {
            Some(any | offered_declares(types, member, declares)?)
        }),
        _ => None,
    }
}

/// The signature a view at `shown` carries, where `shown` is a signature or an application of one
/// pinning every head parameter: what [`view_signature`] gives its pins. `None` where a parameter
/// is left unpinned, or `shown` is no declared signature's application.
pub fn shown_view(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    shown: KType,
) -> Option<KType> {
    let (declared, pins) = match types.node(shown) {
        TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
        _ => (shown, None),
    };
    let TypeNode::Signature { schema, .. } = types.node(declared) else {
        return None;
    };
    let mut to = BumpVec::with_capacity_in(schema.parameters.len(), scratch);
    for (name, _) in schema.parameters.iter() {
        let pin = pins.and_then(|pins| pins.get(BinderSymbol::Type(*name).symbol()))?;
        to.push((*name, pin));
    }
    Some(view_signature(
        &schema,
        Members::from_pairs(scratch, to.iter().copied()),
        types,
        scratch,
    ))
}
