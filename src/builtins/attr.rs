//! `ATTR <s> <field:Identifier>` — record (anonymous or newtype-wrapped), module, or signature
//! field access. Surface syntax is the `.` infix operator. Overloads share the bucket
//! `[Keyword, Slot, Slot]` and pick by lhs shape: [`body_identifier`] for `p.x` where
//! the lhs is still an `Identifier`, [`body_value`] for a runtime-value lhs (an anonymous record's
//! `.x` reads off it directly; a record-repr newtype's reads through the wrap), [`body_module`]
//! for chained module access.
//!
//! The lhs is matched by *type*, never by a kind: a module value picks `body_module` through the
//! empty-signature slot every module's self-sig satisfies, a type-token lhs picks `body_type_lhs`
//! through its `OfKind` kind, and any other value-channel lhs is caught by the least-specific
//! `s: Any` slot and validated in [`access_field`]. Specificity (`Any` < `OfKind` < `Identifier`)
//! resolves the overloads: an `Identifier` lhs wins `body_identifier`, a module / type-token lhs
//! wins its own slot, and only a bare runtime value falls through to [`body_value`].
//!
//! The `field` position does not split. One `NameToken` slot takes a bare field token of either
//! class, and it outranks `Str`, so the spelled forms above always win it; a field that arrives as
//! a runtime string — computed or literal — falls to the dynamic pair, [`body_dynamic_field`] for
//! a value lhs and [`body_dynamic_module_field`] for a module one.
//!
//! The overload table does not split on context either. [`body_type_lhs`] reads the type-sigil
//! stamp off its own [`BodyCtx`](crate::machine::BodyCtx) and hands the projection ladder a
//! [`ProjectionContext`]: a *value*-class field names a type only under `:(…)`, so
//! `:(Ordered.compare)` names the `VAL` slot's declared type and `:(Point.x)` the record-repr
//! newtype's field type, while the bare spellings name no member. A *type*-class field
//! (`Maybe.Some`, `Ordered.Carrier`) is a type name already and projects either way.

use std::borrow::Cow;

use crate::machine::StepAllocator;
use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::model::KKind;
use crate::machine::model::TypeResolution;
use crate::machine::model::{BinderSymbol, Carried, Module, NodeSchema, TypeSymbol};
use crate::machine::model::{CarriedFamily, Held, KObject, KType, PartedCell, TypeNode};
use crate::machine::{KError, KErrorKind, MemberResolution, NameLookup, Scope};

use super::{arg, kw, sig};
use crate::machine::BoundArgs;
use crate::machine::DeliveredCarried;
use crate::machine::model::RunRegistries;
use crate::machine::model::Symbol;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { field, s } }

/// Lift an `access_*` result into its terminal [`Action`]: a projected member — object or type —
/// seals as a [`StepCarried`] carrier naming its reach ([`Action::done(Ok)`]), an error as a
/// [`Action::done(Err)`]. Both channels are witnessed: an object value re-projected at the fold
/// brand from the lhs operand's view (its reach folded by construction), a type identity witnessed
/// in place from its stored reach via [`Scope::resident`] (or, for a projected type
/// field, re-projected and sealed under the folded lhs reach).
fn route<'a>(result: Result<StepCarried<'a>, KError>) -> crate::machine::Action<'a> {
    crate::machine::Action::done(result)
}

/// The `field` member name, carried as the classification of the channel it arrived on rather
/// than as text a consumer re-derives a class from. A member probe reads only [`Self::symbol`],
/// so the spelling is rendered where a diagnostic quotes it and nowhere else.
enum FieldName<'a> {
    /// A name token the parse classified and interned. Its class *is* the channel it arrived on,
    /// which is the map a member probe keys.
    Token(BinderSymbol),
    /// A name that classifies as neither channel: a runtime string that spells no token. It binds
    /// nowhere, so it names no member and every consumer treats it as an immediate miss reporting
    /// this text. The producer borrows the string it read, so the read that succeeds allocates
    /// nothing.
    Rendered(Cow<'a, str>),
}

impl<'a> FieldName<'a> {
    /// The classification the name arrived under, or `None` for a rendering that names no member.
    fn class(&self) -> Option<BinderSymbol> {
        match self {
            FieldName::Token(class) => Some(*class),
            FieldName::Rendered(_) => None,
        }
    }

    /// The bare digest for a runtime data-label probe. A record field is classless (see
    /// [design/label-interning.md](../../design/label-interning.md)), so the lookup keys on the
    /// digest alone — reusing the classification's when the name has one, hashing the text when
    /// it does not.
    fn symbol(&self) -> Symbol {
        match self {
            FieldName::Token(class) => class.symbol(),
            FieldName::Rendered(text) => Symbol::of(text),
        }
    }

    /// The spelling a diagnostic quotes. A classified name renders out of the run's label table
    /// here — the one place the member read needs text at all.
    fn text(&self, registries: &RunRegistries) -> Cow<'_, str> {
        match self {
            FieldName::Token(class) => Cow::Owned(crate::machine::model::render_label(
                class.symbol(),
                registries,
            )),
            FieldName::Rendered(text) => Cow::Borrowed(text.as_ref()),
        }
    }
}

/// Read the `field` member name from `BodyCtx::args`. The slot is a `NameToken`, so a bare field
/// token of either class arrives on one channel already carrying the class the parse assigned it —
/// nothing here lowers a name or re-derives a class by a predicate over text.
fn read_field_name<'a>(args: BoundArgs<'a, '_>) -> Result<FieldName<'a>, KError> {
    match args.name(&SLOTS.field) {
        Some(class) => Ok(FieldName::Token(class)),
        None => Err(KError::new(KErrorKind::MissingArg("field".to_string()))),
    }
}

/// Read the `field` member name off a `:Str` slot — the dynamic read's counterpart to
/// [`read_field_name`]. The slot's type admits no other object shape, so the string arm is the
/// whole vocabulary.
fn read_dynamic_field_name<'a>(
    args: BoundArgs<'a, '_>,
    registries: &RunRegistries,
) -> Result<FieldName<'a>, KError> {
    match args.object(&SLOTS.field) {
        Some(KObject::KString(text)) => Ok(classify_derived_field(text, registries)),
        Some(_) | None => Err(KError::new(KErrorKind::MissingArg("field".to_string()))),
    }
}

/// Classify a member name that reached the read as text rather than as a token the parse minted:
/// the runtime string [`read_dynamic_field_name`] reads. This is the
/// value channel's one derived-symbol door: [`BinderSymbol::declared`] classifies and interns in
/// one step, so a spelling read off text keys the same symbol a bare token of that spelling would
/// have minted. Interning here is what widens the label table past the run's source text — see
/// [design/label-interning.md](../../design/label-interning.md). Text that classifies as neither
/// channel names no binding, so it rides as a rendering — a digest-keyed record probe and an
/// immediate module miss.
fn classify_derived_field<'a>(text: &'a str, registries: &RunRegistries) -> FieldName<'a> {
    match BinderSymbol::declared(text, &registries.labels) {
        Some(class) => FieldName::Token(class),
        None => FieldName::Rendered(Cow::Borrowed(text)),
    }
}

/// Value-then-type lookup of the `s` identifier against `ctx.scope`, returning the projected
/// member as `Action::Done`. A module-valued parameter binds value-side, so a lowercase
/// (Identifier-classed) parameter member access like `elem.compare` inside a functor body reaches
/// the module through the value arm. The type-side probe serves a name bound to an abstract
/// identity (a SIG value slot's `VAL zero :Carrier` type), which names no receiver to project off.
pub fn body_identifier<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    let Some(s_name) = ctx.args.identifier(&SLOTS.s) else {
        return Action::done(Err(KError::new(KErrorKind::MissingArg("s".to_string()))));
    };
    let field_name = crate::try_action!(read_field_name(ctx.args));
    // `s` is a bound name: cross the binding's own carrier as the field read's lhs operand, so the
    // projected field folds every region the bound value reaches. The lift is the only read — the
    // field probe runs under the envelope's own pins rather than off a bare reference.
    // The type channel is not consulted here: a value-classified token names no type, and a
    // Type-token lhs picks `body_type_lhs` through its own slot.
    if let Some(lhs) = ctx.scope.lookup_value_delivered(s_name) {
        return route(access_field(&ctx.ctx, &field_name, &lhs, ctx.registries));
    }
    Action::done(Err(KError::new(KErrorKind::UnboundName(
        crate::machine::model::render_label(s_name.symbol(), ctx.registries),
    ))))
}

/// `ATTR <s:ProperType> <field:_>` — entry for a type-channel lhs, e.g. a first-class signature
/// value (see [token classes](../../design/typing/tokens.md) for why such an lhs token is
/// Type-classed). The Type-Type overload shares this body so a chained access whose field is itself
/// a Type token reaches the same projection. Projects a member off the Type-classed `s`, resolving
/// an unlowered name carrier through the memoized bridge first. A module lhs rides the value channel
/// and picks [`body_module`] instead, so `Foo.Carrier` projects off the module value.
pub fn body_type_lhs<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    let context = ProjectionContext::of(ctx.under_type_sigil);
    if let Some(te) = ctx.args.unresolved_type(&SLOTS.s) {
        let field_name = crate::try_action!(read_field_name(ctx.args));
        return match ctx.scope.resolve_type_identifier(te, None, ctx.registries) {
            TypeResolution::Done(kt) => route(access_type_member(
                ctx.scope,
                kt,
                Some(te),
                &field_name,
                context,
                ctx.registries,
            )),
            TypeResolution::Unbound(name) => {
                Action::done(Err(KError::new(KErrorKind::UnboundName(
                    crate::machine::model::render_label(name.symbol(), ctx.registries),
                ))))
            }
            TypeResolution::Park(producers) => {
                Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "ATTR lhs type `{}` resolved to a still-finalizing type \
                     (parked on {} producer(s)); the type argument should already be sealed \
                     at body entry",
                    crate::machine::model::render_label(te.symbol(), ctx.registries),
                    producers.len(),
                )))))
            }
        };
    }
    let s_kt = match ctx.args.ktype(&SLOTS.s) {
        Some(kt) => kt,
        None => {
            return Action::done(Err(match ctx.args.object(&SLOTS.s) {
                Some(other) => KError::new(KErrorKind::TypeMismatch {
                    arg: "s".to_string(),
                    expected: "ProperType".to_string(),
                    got: other.ktype().name(ctx.registries),
                }),
                None => KError::new(KErrorKind::MissingArg("s".to_string())),
            }));
        }
    };
    let field_name = crate::try_action!(read_field_name(ctx.args));
    route(access_type_member(
        ctx.scope,
        s_kt,
        None,
        &field_name,
        context,
        ctx.registries,
    ))
}

/// Reads the runtime-value lhs and projects the field through [`access_field`].
pub fn body_value<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    let target = match ctx.args.object(&SLOTS.s) {
        Some(obj) => obj,
        None => return Action::done(Err(KError::new(KErrorKind::MissingArg("s".to_string())))),
    };
    let field_name = crate::try_action!(read_field_name(ctx.args));
    // The lhs `s` is a computed `Wrapped` value delivered to this call (e.g. `seg.finish.x`), so its
    // carrier names regions the read-site frame may not pin; cross the lhs carrier as the field
    // read's operand so the projected field outlives every region the lhs reaches. A carrier-less
    // `s` is region-pure by the argument view's carrier contract, so it is placed into the
    // read-site region through the shape-split pure door and enveloped there —
    // coverage-equivalent to an empty-reach seal. No region-pure shape is a `Wrapped`, so that
    // arm's diagnostic is what a construction bug would surface here.
    match ctx.args.carrier(&SLOTS.s) {
        Some(lhs) => route(access_field(&ctx.ctx, &field_name, lhs, ctx.registries)),
        None => {
            let resident = match ctx.scope.deliver_pure_value(target) {
                Ok(resident) => resident,
                Err(e) => return Action::done(Err(e)),
            };
            route(access_field(
                &ctx.ctx,
                &field_name,
                &resident,
                ctx.registries,
            ))
        }
    }
}

/// `ATTR <s:Any> <field:Str>` — the dynamic member read, where the name arrives as a runtime
/// string instead of a token the parse classified. `Any` + `Str` is the least specific shape in the
/// bucket and `Identifier` outranks `Str`, so a bare field token always picks a sibling and only a
/// computed or literal string reaches here.
///
/// The name is classified and interned at the read ([`classify_derived_field`]), so `s."x"` probes
/// the same symbol `s.x` does. The lhs is read the way [`body_value`] reads its own `s :Any`
/// slot; a type-channel lhs names no runtime member and errors. A module lhs picks
/// [`body_dynamic_module_field`] through the more specific empty-signature slot, the same split the
/// bare-token overloads make.
pub fn body_dynamic_field<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    let field_name = crate::try_action!(read_dynamic_field_name(ctx.args, ctx.registries));
    let target = match ctx.args.held(&SLOTS.s) {
        Some(Held::Object(obj)) => obj,
        Some(Held::Type(_) | Held::UnresolvedType(_)) => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`ATTR <s> <field :Str>` reads a member off a runtime value; a type-channel lhs \
                 has no member named by a string (`{}`)",
                field_name.text(ctx.registries),
            )))));
        }
        // The `s` slot is `:Any`, which admits no raw part capture.
        Some(Held::Name(_) | Held::RecordType(_)) => {
            unreachable!("ATTR's lhs slot never captures a raw part")
        }
        None => return Action::done(Err(KError::new(KErrorKind::MissingArg("s".to_string())))),
    };
    // Same operand contract as `body_value`: a delivered lhs crosses as the read's operand so the
    // projected member outlives every region the lhs reaches, and a carrier-less region-pure lhs is
    // placed through the shape-split pure door instead.
    match ctx.args.carrier(&SLOTS.s) {
        Some(lhs) => route(access_field(&ctx.ctx, &field_name, lhs, ctx.registries)),
        None => {
            let resident = match ctx.scope.deliver_pure_value(target) {
                Ok(resident) => resident,
                Err(e) => return Action::done(Err(e)),
            };
            route(access_field(
                &ctx.ctx,
                &field_name,
                &resident,
                ctx.registries,
            ))
        }
    }
}

/// Projects the field off a module lhs riding the value channel's Object arm.
pub fn body_module<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    let m = crate::try_action!(module_lhs(ctx.args, ctx.registries));
    let field_name = crate::try_action!(read_field_name(ctx.args));
    route(access_module_member(m, &field_name, ctx.registries))
}

/// `ATTR <s:EmptySignature> <field:Str>` — [`body_module`]'s dynamic read. The empty-signature slot
/// every module's self-sig satisfies outranks [`body_dynamic_field`]'s `Any`, so a module lhs with a
/// string field lands here and answers out of the module's own bindings rather than missing against
/// the record walk.
pub fn body_dynamic_module_field<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    let m = crate::try_action!(module_lhs(ctx.args, ctx.registries));
    let field_name = crate::try_action!(read_dynamic_field_name(ctx.args, ctx.registries));
    route(access_module_member(m, &field_name, ctx.registries))
}

/// The `s` slot as the [`Module`] the empty-signature match promised. Both module bodies enter
/// through here; the arms below are what a mismatch between that promise and the bound cell would
/// surface.
fn module_lhs<'a>(
    args: BoundArgs<'a, '_>,
    registries: &RunRegistries,
) -> Result<&'a Module<'a>, KError> {
    match args.object(&SLOTS.s) {
        Some(KObject::Module(module)) => Ok(*module),
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "Module".to_string(),
            got: other.ktype().name(registries),
        })),
        None => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "s".to_string(),
            expected: "Module".to_string(),
            got: "Type".to_string(),
        })),
    }
}

/// Which universe a type-lhs projection is being read in — the type-sigil stamp
/// ([`BodyCtx::under_type_sigil`](crate::machine::BodyCtx)) as the vocabulary the projection ladder
/// takes it in.
///
/// A *type*-class field names a type in either context: `Maybe.Some` and `Ordered.Carrier` project
/// bare and under the sigil alike, because the name that spells them is already a type name. A
/// *value*-class field is the split. Token class is a binding rule
/// ([tokens.md](../../design/typing/tokens.md)), so a value token names a type only where the
/// surface says so, and `:(…)` is that surface: `:(Ordered.compare)` names the `VAL` slot's
/// declared type, while bare `Ordered.compare` names no member at all.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProjectionContext {
    /// An ordinary value-context expression. A value-class field names no member here.
    Value,
    /// Under the type sigil `:(…)`. A value-class field names its declared type.
    Type,
}

impl ProjectionContext {
    fn of(under_type_sigil: bool) -> Self {
        if under_type_sigil {
            ProjectionContext::Type
        } else {
            ProjectionContext::Value
        }
    }

    /// May `field` name a type here? Only the sigil admits a value-class name into the type
    /// universe; a type-class name is one already and needs no surface to say so.
    fn admits(self, field: &FieldName<'_>) -> bool {
        !matches!(field.class(), Some(BinderSymbol::Value(_))) || self == ProjectionContext::Type
    }
}

/// Project `field` off a Type-channel lhs. A signature answers directly from its owned schema —
/// a manifest or abstract type member first, then, under the type sigil, a declared value slot's
/// type — with no decl-scope reverse-lookup; a record-repr newtype answers its sealed record
/// schema's field type under the same sigil rule; a union answers the variant `field` names, which
/// is the one door to a variant (`Maybe.Some` names it, `Maybe.Some 42` constructs through it); an
/// abstract identity carries no receiver and errors. A module rides the value channel, so a module
/// lhs lands in [`body_module`] instead.
fn access_type_member<'a>(
    scope: &Scope<'a>,
    kt: KType,
    spelling: Option<TypeSymbol>,
    field: &FieldName<'_>,
    context: ProjectionContext,
    registries: &RunRegistries,
) -> Result<StepCarried<'a>, KError> {
    /// What the lhs node offers this read, decided under the registry borrow so the node is
    /// read in place rather than cloned out: a signature answers the member handle it keys (or
    /// the miss), an abstract identity answers its own name, and every other node carries no
    /// members at all. Each arm is a `Copy` payload, so nothing borrows past the read.
    enum Projection {
        Signature(Option<KType>),
        /// A union node: its variants are its members, and the lookup rule needs the reading
        /// scope, so this arm carries the decision out of the borrow rather than answering under it.
        Union,
        /// A nominal over a transparent representation. The fields live in *that* node, so the
        /// handle rides out of this borrow and the record read runs in a second one.
        NewTypeRepr(KType),
        Abstract(TypeSymbol),
        NoMembers,
    }
    let projection = registries.types.with_node(kt, |node| match node {
        // ATTR over a first-class signature value — answered from the owned schema, which the
        // read borrows rather than cloning; the member handle it names is copied out.
        TypeNode::Signature { schema, .. } => {
            // The field arrives already classified by the channel it came in on, and the schema's
            // partition is keyed by that same classification: a Type-class name can key only the
            // type-member maps, a Value-class one only the value slots. An unclassifiable name
            // keys neither and falls to the no-member error below.
            let member = match field.class() {
                Some(BinderSymbol::Type(name)) => schema
                    .manifest_members
                    .get(&name)
                    .or_else(|| schema.abstract_members.get(&name)),
                Some(BinderSymbol::Value(name)) => schema.value_slots.get(&name),
                None => None,
            };
            Projection::Signature(member.copied())
        }
        TypeNode::Union { .. } => Projection::Union,
        TypeNode::SetMember {
            schema: NodeSchema::NewType(repr),
            ..
        } => Projection::NewTypeRepr(*repr),
        TypeNode::AbstractType { name, .. } => Projection::Abstract(*name),
        _ => Projection::NoMembers,
    });
    match projection {
        // The member type is allocated fresh into the read-site scope's own region. A value-class
        // field is a `VAL` slot's declared type, which only the sigiled surface names.
        Projection::Signature(Some(member)) if context.admits(field) => {
            Ok(StepCarried::born(scope.resident(Carried::Type(member))))
        }
        Projection::Signature(member) => Err(sigil_hinted(
            KError::new(KErrorKind::ShapeError(format!(
                "signature `{}` has no member `{}`",
                lhs_spelling(spelling, kt, registries),
                field.text(registries)
            ))),
            member.is_some(),
            spelling,
            kt,
            field,
            registries,
        )),
        Projection::NewTypeRepr(repr) => {
            record_repr_member(scope, kt, repr, spelling, field, context, registries)
        }
        // A variant reference reads against the member list — the same rule a `MATCH … OVER` arm
        // head reads by — so the two surfaces can never disagree about what names a variant.
        Projection::Union => match crate::builtins::union::union_member(
            scope,
            kt,
            field.symbol(),
            match field.class() {
                Some(BinderSymbol::Type(name)) => Some(name),
                _ => None,
            },
            registries,
        ) {
            Some(member) => Ok(StepCarried::born(scope.resident(Carried::Type(member)))),
            None => Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` is not a member of `{}` (members: {})",
                field.text(registries),
                lhs_spelling(spelling, kt, registries),
                crate::builtins::union::union_member_names(kt, registries),
            )))),
        },
        Projection::Abstract(name) => Err(abstract_type_has_no_members(
            &crate::machine::model::render_label(name.symbol(), registries),
        )),
        Projection::NoMembers => Err(KError::new(KErrorKind::ShapeError(format!(
            "type `{}` has no member `{}`",
            kt.name(registries),
            field.text(registries)
        )))),
    }
}

/// The lhs as a diagnostic names it: the name the read was spelled with when it had one, and the
/// node's own rendering otherwise. A user `UNION` binds an *anonymous* union, so the node renders
/// structurally and names nothing the source wrote.
fn lhs_spelling(spelling: Option<TypeSymbol>, kt: KType, registries: &RunRegistries) -> String {
    match spelling {
        Some(name) => crate::machine::model::render_label(name.symbol(), registries),
        None => kt.display_name(registries).to_string(),
    }
}

/// Append the sigiled spelling to a value-context miss the sigil *would* have answered — the one
/// refinement the context split adds to the diagnostics. A miss the sigil would not rescue carries
/// no hint: `Number.foo` names a memberless type and `Maybe.some` names no variant, and neither
/// has a spelling that would answer.
fn sigil_hinted(
    error: KError,
    would_hit_under_sigil: bool,
    spelling: Option<TypeSymbol>,
    kt: KType,
    field: &FieldName<'_>,
    registries: &RunRegistries,
) -> KError {
    if !would_hit_under_sigil {
        return error;
    }
    let KErrorKind::ShapeError(message) = &error.kind else {
        return error;
    };
    KError::new(KErrorKind::ShapeError(format!(
        "{message} — write `:({}.{})` to name the field's declared type",
        lhs_spelling(spelling, kt, registries),
        field.text(registries),
    )))
}

/// Project `field` off the sealed record schema a record-repr newtype wraps — the product-side peer
/// of the signature arm's `VAL` slot read, under the same sigil rule. `repr` is read in its own
/// registry borrow, which is why [`access_type_member`]'s ladder carries the handle out rather than
/// answering under the lhs node's borrow.
///
/// A newtype over anything else (`NEWTYPE Meters = Number`) has no fields to name and takes the
/// memberless-type error.
fn record_repr_member<'a>(
    scope: &Scope<'a>,
    kt: KType,
    repr: KType,
    spelling: Option<TypeSymbol>,
    field: &FieldName<'_>,
    context: ProjectionContext,
    registries: &RunRegistries,
) -> Result<StepCarried<'a>, KError> {
    /// The representation as this read sees it, carried out of the borrow: the field's declared
    /// type when the record has one, alongside the record's own field names for the miss.
    enum Repr {
        Record(Option<KType>, Vec<Symbol>),
        Scalar,
    }
    let repr = registries.types.with_node(repr, |node| match node {
        TypeNode::Record { fields } => Repr::Record(
            fields.get(field.symbol()).copied(),
            fields.keys().map(|name| name.symbol()).collect(),
        ),
        _ => Repr::Scalar,
    });
    let no_member = || {
        KError::new(KErrorKind::ShapeError(format!(
            "type `{}` has no member `{}`",
            kt.name(registries),
            field.text(registries)
        )))
    };
    match repr {
        Repr::Record(Some(declared), _) if context.admits(field) => {
            Ok(StepCarried::born(scope.resident(Carried::Type(declared))))
        }
        Repr::Record(declared, names) => match context {
            // The sigil asked for a field type and the record carries no such field: name what it
            // does carry, the way a union names its variants on a member miss.
            ProjectionContext::Type => Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` is not a member of `{}` (fields: {})",
                field.text(registries),
                lhs_spelling(spelling, kt, registries),
                names
                    .iter()
                    .map(|name| crate::machine::model::render_label(*name, registries))
                    .collect::<Vec<_>>()
                    .join(", "),
            )))),
            ProjectionContext::Value => Err(sigil_hinted(
                no_member(),
                declared.is_some(),
                spelling,
                kt,
                field,
                registries,
            )),
        },
        Repr::Scalar => Err(no_member()),
    }
}

/// An abstract type ([`TypeNode::AbstractType`]) is an identity — a binder scope, a name, and a generativity nonce —
/// not a receiver. The
/// module it names rides the value channel, and further members project off *that* value, so a
/// member access whose lhs is a bare abstract identity has nowhere to look.
fn abstract_type_has_no_members(name: &str) -> KError {
    KError::new(KErrorKind::ShapeError(format!(
        "abstract type `{name}` has no projectable members here — project off the module value"
    )))
}

/// Walk any `Wrapped` layers to the record member named `field` and **part it from its
/// container**: the cell arrives bundled with exactly its own run's stored reach, read off the run,
/// never derived by a subset walk over the container. Lifetime-generic in the container's own region,
/// so the parted cell is confined there until a relocation seam lifts it out.
///
/// Two shapes carry fields and both project off the same `RecordSubstrate`. An anonymous record
/// value is read directly, and its miss renders the structural type it carries
/// (`` `:{x :Number}` has no field `z` ``). A record-repr newtype (an ex-struct) wraps a
/// `KObject::Record`; the member reads through the wrap, naming the nominal type in the miss so
/// `b.z` on a `Point` reports `Point`. `Wrapped.inner` is invariantly not a `Wrapped` (the
/// construction-time collapse rule peels any `Wrapped` before re-wrapping), so a scalar inner
/// (a NEWTYPE-over-`Number`, which has no fields) falls to the `other` arm.
fn wrapped_field_cell<'w>(
    target: &'w KObject<'w>,
    field: &FieldName<'_>,
    registries: &RunRegistries,
) -> Result<PartedCell<'w>, KError> {
    match target {
        KObject::Wrapped { inner, type_id } => match inner.payload() {
            KObject::Record(substrate, _) => match substrate.field_index(field.symbol()) {
                Some(at) => Ok(substrate
                    .project(at)
                    .expect("the index came from this substrate's own layout")),
                None => Err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}` has no field `{}`",
                    type_id.display_name(registries),
                    field.text(registries)
                )))),
            },
            payload => wrapped_field_cell(payload, field, registries),
        },
        // An anonymous record value reads the same way, one layer shallower: the member parts off
        // the substrate directly and the miss renders the value's structural type, since there is
        // no nominal layer to name.
        //
        // The *carried type* decides which fields the read admits, the same currency dispatch
        // reads by. `FROM` narrows that type while sharing the substrate whole, so a
        // projected-away field is unreadable through the view even though its cell is still
        // physically present. The substrate then supplies the cell for a field the type admits —
        // the two agree on every field the type names.
        KObject::Record(substrate, record_type) => {
            let admitted = registries.types.with_node(*record_type, |node| match node {
                TypeNode::Record { fields } => fields.get(field.symbol()).is_some(),
                // A record value's handle is interned over its own fields at construction and
                // re-stamped only to another record type, so no other node carries one.
                _ => false,
            });
            match substrate.field_index(field.symbol()).filter(|_| admitted) {
                Some(at) => Ok(substrate
                    .project(at)
                    .expect("the index came from this substrate's own layout")),
                None => Err(KError::new(KErrorKind::ShapeError(format!(
                    "`{}` has no field `{}`",
                    record_type.name(registries),
                    field.text(registries)
                )))),
            }
        }
        other => Err(KError::new(KErrorKind::ShapeError(format!(
            "cannot read field `{}` off a `{}` value — it has no fields",
            field.text(registries),
            other.ktype().name(registries)
        )))),
    }
}

/// Project `field` off the runtime-value lhs whose carrier is the declared operand `lhs`.
///
/// The member is **parted** from its container ([`wrapped_field_cell`]) under the envelope's own
/// pins, so it arrives paired with exactly its own run's stored reach rather than the whole
/// container's. [`Opened::lift_out`](crate::witnessed::Opened::lift_out) is the relocation seam that
/// turns that run into owned coverage — the run's members plus the region the cell lives in, and
/// nothing else — and the field carrier is then folded from *that* envelope, so the product names
/// exactly what the member reaches instead of everything the container did.
///
/// A shallow-scalar member embeds no borrow, so it seals with an empty reach through the no-fold
/// door; a type member is owned data that clones into the read site's own region.
fn access_field<'a>(
    step: &StepAllocator<'a>,
    field: &FieldName<'_>,
    lhs: &DeliveredCarried,
    registries: &RunRegistries,
) -> Result<StepCarried<'a>, KError> {
    // The open borrows the envelope's own pins for the whole read, which covers both the walk and
    // the `lift_out` upgrade below.
    let opened = lhs.open_at();
    let parted = wrapped_field_cell(opened.value().object(), field, registries)?;
    match parted.value() {
        // A type member is owned data: it clones out of the container and allocates into the read
        // site's own region, so the read carries no dependence on the lhs carrier.
        Held::Type(kt) => return Ok(step.type_carried(*kt)),
        // A record field cell is a value or a resolved type; the bind seam's unlowered carrier
        // never lands in one.
        Held::UnresolvedType(_) => unreachable!("a record field is never an unlowered type name"),
        // A raw-capture carrier is minted only at a part-kind-exact slot of the bind seam; a
        // member cell comes from a container, which never holds one.
        Held::Name(_) | Held::RecordType(_) => {
            unreachable!("a record field is never a raw part capture")
        }
        Held::Object(value) => {
            // A shallow scalar embeds no borrow at all, so it rebuilds owned and seals empty rather
            // than naming the member's run.
            if let Some(sealed) = step.alloc_object_scalar(value) {
                return Ok(sealed);
            }
        }
    }
    // Re-family the lifted cell onto the value channel in place: the envelope keeps the coverage the
    // lift derived, and the projection selects a part of the cell it already covers.
    let member = parted.lift_out().project::<CarriedFamily>(|cell, _token| {
        Carried::Object(
            cell.as_object()
                .expect("classified above: the member is an object cell"),
        )
    });
    Ok(step.alloc_carried_with(&[&member], |b, views| {
        Carried::Object(b.alloc_object_folded(views[0].object().deep_clone()))
    }))
}

/// Look `field` up inside a [`Module`]'s child scope: opaque-ascription `type_members`,
/// then the classified `data`-then-`types` member lookup ([`Bindings::lookup_member`]).
///
/// A nested `MODULE sub = (...)` is a value member, so chained access `Outer.Inner.X` reads the
/// inner module value from `data` and the next ATTR step recurses into its child scope.
///
/// On a value-side hit, an opaque-ascription `slot_type_tags` entry re-tags the read: the
/// raw value is rewrapped in a `KObject::Wrapped` carrier whose `ktype()` is the per-call
/// abstract identity the SIG named (so `(int_ord.zero)` reads as the view's nonced
/// `AbstractType` for `Type`, not the underlying `Number`). Transparent `:!` leaves `slot_type_tags` empty,
/// so transparent reads stay concrete.
///
/// The re-tag carrier is alloc'd in the *module*'s region, not the read-site `scope`'s:
/// `inner` is a pre-existing reference into the module region, so the wrapper is built
/// beside it under the module's home frame, which transitively pins the module's
/// reach-set for the read value. (`type_id` is a Copy handle and imposes no placement
/// constraint.)
fn access_module_member<'a>(
    m: &'a Module<'a>,
    field: &FieldName<'_>,
    registries: &RunRegistries,
) -> Result<StepCarried<'a>, KError> {
    let module_scope = m.child_scope();
    // The field carries the classification of the channel it arrived on; the arms below probe the
    // map that keys by that class, and a field naming neither class (a keyword-class token) is
    // `no member` by the same route a wrong-class name is.
    let no_member = || {
        KError::new(KErrorKind::ShapeError(format!(
            "module `{}` has no member `{}`",
            m.path,
            field.text(registries)
        )))
    };
    let Some(binder) = field.class() else {
        return Err(no_member());
    };
    if let BinderSymbol::Type(name) = binder
        && let Some(minted) = m.type_members.get(&name).copied()
    {
        // Prefer the child scope's own binding; a member present only in the mirror is an
        // `:|`-minted abstract type.
        return Ok(StepCarried::born(
            match module_scope.bindings().lookup_type(name, None) {
                Some(NameLookup::Bound(kt)) => module_scope.resident(Carried::Type(kt)),
                _ => module_scope.resident(Carried::Type(minted)),
            },
        ));
    }
    // One classified lookup over the module's own bindings — the cross-kind exclusion makes a
    // name value-xor-type, so a single read decides the arm instead of probing `data` then
    // `types` by hand. A value member lives in the module's region; it seals under the module
    // scope's home frame, which transitively pins the module's reach-set — so the read value
    // (or its re-tag carrier) names the full reach without an embedded lhs to fold (the module
    // identity is the lhs).
    match module_scope.bindings().lookup_member(binder, None) {
        Some(MemberResolution::Value(sealed)) => {
            let tag = match binder {
                BinderSymbol::Value(name) => m.slot_type_tags.get(&name).copied(),
                BinderSymbol::Type(_) => None,
            };
            if let Some(tag) = tag {
                // The re-tag allocates in the module region (not the read site's): both the value
                // member and the re-tag identity `tag` cross as declared fold operands. The member
                // is a binding seal lifted into an envelope pinned by the module scope's own region
                // owner; `tag` is a Copy handle sealed resident via `Scope::resident`. Both
                // carriers union into the wrapped result's witness via `alloc_carried_with`.
                let obj_carrier = module_scope.lift_resident(sealed);
                let tag_carrier = module_scope.deliver_resident(Carried::Type(tag));
                let ctx = StepAllocator::for_scope(module_scope);
                // The peel keeps the member's payload verbatim, so a payload substrate that stays
                // in the module's region rides as the payload cell's own stored run; the member
                // carrier's coverage is the holder-rule proof for reading it at the door.
                let holder = obj_carrier.coverage().clone();
                return Ok(ctx.alloc_carried_with(
                    &[&obj_carrier, &tag_carrier],
                    |b, views| match (views[0], views[1]) {
                        (Carried::Object(o), Carried::Type(tag)) => {
                            Carried::Object(b.alloc_object_folded(KObject::wrapped_peel(
                                b.with_holder(&holder),
                                o,
                                tag,
                            )))
                        }
                        _ => unreachable!("operand order: [value member, re-tag identity]"),
                    },
                ));
            }
            // A value member read reaches into the module's region; the lift upgrades the member's
            // exact reach into the owned pins that ride the step.
            Ok(StepCarried::born_delivered(
                module_scope.lift_resident(sealed),
            ))
        }
        Some(MemberResolution::Type { kt }) => {
            Ok(StepCarried::born(module_scope.resident(Carried::Type(kt))))
        }
        None => Err(no_member()),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let identifier_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "ATTR"),
                arg(registries, &SLOTS.s, KType::IDENTIFIER),
                arg(registries, &SLOTS.field, KType::NAME_TOKEN),
            ],
        )
    };
    let module_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "ATTR"),
                arg(registries, &SLOTS.s, KType::EMPTY_SIGNATURE),
                arg(registries, &SLOTS.field, KType::NAME_TOKEN),
            ],
        )
    };
    // Runtime-value fall-through: a bare record and a NEWTYPE wrap (ex-structs included). A
    // computed lhs (e.g. `seg.finish.x`) arrives in the Object channel; the `s: Any` slot matches
    // the *value* by a type (never by a kind — `OfKind` is type-channel-only), and `access_field`
    // validates the shape, reading a bare record's field off its substrate, a wrapped record
    // repr's through one layer, and recursing for any other inner — a value with no fields errors
    // naming the field and the operand's type. This stays unambiguous with the sibling overloads:
    // `Any` is the least specific, so an `Identifier` lhs picks `body_identifier`, a module /
    // type-token lhs picks `body_module` / `body_type_lhs`, and only a bare runtime value falls
    // through to here.
    let value_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "ATTR"),
                arg(registries, &SLOTS.s, KType::ANY),
                arg(registries, &SLOTS.field, KType::NAME_TOKEN),
            ],
        )
    };
    // The dynamic member read: a field name computed at runtime rather than spelled as a token.
    // `Any` + `Str` loses to every sibling for a bare field token (`IDENTIFIER` outranks `STR`), so
    // this overload fires only when the field position produced a string.
    let dynamic_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "ATTR"),
                arg(registries, &SLOTS.s, KType::ANY),
                arg(registries, &SLOTS.field, KType::STR),
            ],
        )
    };
    let type_identifier_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "ATTR"),
                arg(registries, &SLOTS.s, KType::of_kind(KKind::ProperType)),
                arg(registries, &SLOTS.field, KType::NAME_TOKEN),
            ],
        )
    };
    // Module lhs with a computed field name. The empty-signature slot outranks the dynamic read's
    // `Any`, so `m."x"` answers out of the module's bindings instead of the record walk.
    let dynamic_module_field_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "ATTR"),
                arg(registries, &SLOTS.s, KType::EMPTY_SIGNATURE),
                arg(registries, &SLOTS.field, KType::STR),
            ],
        )
    };

    use crate::builtins::register_builtin;
    register_builtin(scope, identifier_sig(), body_identifier, registries, gate);
    register_builtin(scope, module_field_sig(), body_module, registries, gate);
    register_builtin(scope, value_sig(), body_value, registries, gate);
    register_builtin(
        scope,
        dynamic_field_sig(),
        body_dynamic_field,
        registries,
        gate,
    );
    register_builtin(
        scope,
        dynamic_module_field_sig(),
        body_dynamic_module_field,
        registries,
        gate,
    );
    register_builtin(
        scope,
        type_identifier_field_sig(),
        body_type_lhs,
        registries,
        gate,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::model::KObject;
    use crate::machine::model::KType;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;

    /// A field position captures its token, so a name that happens to spell a primitive is read
    /// exactly like any other Type-classed field: it names no member of `Ordered`, and the miss
    /// reports the token as written.
    #[test]
    fn attr_with_a_primitive_type_field_reports_the_miss() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG Ordered = ((TYPE Carrier) (VAL compare :Number))");
        let err = test_run.run_one_err(test_run.parse_one("Ordered.Str"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(m)
                if m.contains("has no member `Str`")),
            "expected the miss to name the rendered field, got {err}",
        );
    }

    /// A string literal in the field position reaches the same member the bare token names: the
    /// text is classified and interned at the read, so it probes the symbol `p.x` would have.
    #[test]
    fn attr_with_a_string_field_reaches_the_member() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        let result = test_run.run_one(test_run.parse_one("ATTR p \"x\""));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    /// The name need not be a literal: a bound string computes the member to read.
    #[test]
    fn attr_with_a_computed_string_field_reaches_the_member() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             LET p = (Point {x = 3, y = 4})\n\
             LET name_var = \"y\"",
        );
        let result = test_run.run_one(test_run.parse_one("ATTR p (name_var)"));
        assert!(matches!(result, KObject::Number(n) if *n == 4.0));
    }

    /// The specificity flip's behavior pin: a bare field token binds bare whatever else that
    /// spelling names. `x` is bound to a string here, and `p.x` still reads the member `x` rather
    /// than dereferencing the binding through the `:Str` overload.
    #[test]
    fn a_bare_field_token_outranks_a_string_binding_of_the_same_name() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             LET p = (Point {x = 3, y = 4})\n\
             LET x = \"y\"",
        );
        assert!(
            matches!(test_run.run_one(test_run.parse_one("p.x")), KObject::Number(n) if *n == 3.0),
            "`p.x` reads member `x`, not the member named by the binding `x`",
        );
        assert!(
            matches!(test_run.run_one(test_run.parse_one("ATTR p x")), KObject::Number(n) if *n == 3.0),
            "the spelled-out form routes identically",
        );
    }

    /// A dynamic name naming no member misses the same way a bare token does.
    #[test]
    fn attr_with_an_unknown_string_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        let err = test_run.run_one_err(test_run.parse_one("ATTR p \"z\""));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(m)
                if m.contains("`Point` has no field `z`")),
            "expected the ordinary no-member miss, got {err}",
        );
    }

    /// A module answers a computed member name out of its own bindings — the dynamic read's
    /// module split, reached because the empty-signature slot outranks the value read's `Any`.
    #[test]
    fn a_module_lhs_reads_a_member_named_by_a_string() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE m = ((LET x = 7))");
        let result = test_run.run_one(test_run.parse_one("ATTR m \"x\""));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// The name need not be a literal there either.
    #[test]
    fn a_module_lhs_reads_a_member_named_by_a_computed_string() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE m = ((LET x = 7))\nLET name_var = \"x\"");
        let result = test_run.run_one(test_run.parse_one("ATTR m (name_var)"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    /// A computed name naming no module member misses the same way a bare token does.
    #[test]
    fn a_module_lhs_with_an_unknown_string_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE m = ((LET x = 7))");
        let err = test_run.run_one_err(test_run.parse_one("ATTR m \"ghost\""));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("has no member `ghost`")),
            "expected the ordinary module miss, got {err}",
        );
    }

    /// A module lhs with a bare field token still picks the module overload: the empty-signature
    /// slot outranks the dynamic read's `Any`.
    #[test]
    fn a_module_lhs_with_a_bare_field_still_routes_to_the_module_overload() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("MODULE m = ((LET x = 7))");
        let result = test_run.run_one(test_run.parse_one("m.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }

    #[test]
    fn attr_reads_field_from_named_struct() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        let result = test_run.run_one(test_run.parse_one("p.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    #[test]
    fn attr_reads_each_field_independently() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        assert!(
            matches!(test_run.run_one(test_run.parse_one( "p.x")), KObject::Number(n) if *n == 3.0)
        );
        assert!(
            matches!(test_run.run_one(test_run.parse_one( "p.y")), KObject::Number(n) if *n == 4.0)
        );
    }

    #[test]
    fn attr_chained_through_nested_struct() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Line = :{start :Point, finish :Point}\n\
             LET origin = (Point {x = 0, y = 0})\n\
             LET tip = (Point {x = 3, y = 4})\n\
             LET seg = (Line {start = origin, finish = tip})",
        );
        let result = test_run.run_one(test_run.parse_one("seg.finish.x"));
        assert!(matches!(result, KObject::Number(n) if *n == 3.0));
    }

    #[test]
    fn attr_unbound_name_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one("ghost.x"));
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "ghost"),
            "expected UnboundName(\"ghost\"), got {err}",
        );
    }

    #[test]
    fn attr_on_non_struct_value_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET n = 5");
        let err = test_run.run_one_err(test_run.parse_one("n.x"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("`x`") && msg.contains("`Number`") && !msg.contains("'s'")),
            "expected a ShapeError naming the field and the operand type, got {err}",
        );
    }

    #[test]
    fn attr_unknown_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Number}\nLET p = (Point {x = 3, y = 4})");
        let err = test_run.run_one_err(test_run.parse_one("p.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`z`")),
            "expected ShapeError naming Point and z, got {err}",
        );
    }

    #[test]
    fn attr_chained_unknown_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Line = :{start :Point, finish :Point}\n\
             LET origin = (Point {x = 0, y = 0})\n\
             LET tip = (Point {x = 3, y = 4})\n\
             LET seg = (Line {start = origin, finish = tip})",
        );
        let err = test_run.run_one_err(test_run.parse_one("seg.start.bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Point") && msg.contains("`bogus`")),
            "expected ShapeError naming Point and bogus on chained access, got {err}",
        );
    }

    /// `b.x` on a NEWTYPE-wrapped record-newtype reads through to the underlying field.
    #[test]
    fn access_field_falls_through_wrapped_record_newtype() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point {x = 1, y = 2})\n\
             LET b = (Boxed (p))",
        );
        assert!(
            matches!(test_run.run_one(test_run.parse_one( "b.x")), KObject::Number(n) if *n == 1.0)
        );
        assert!(
            matches!(test_run.run_one(test_run.parse_one( "b.y")), KObject::Number(n) if *n == 2.0)
        );
    }

    /// Wrapping a scalar doesn't grow fields: `d.x` on a NEWTYPE-over-Number errors.
    #[test]
    fn access_field_rejects_wrapped_non_struct() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Distance = Number\n\
             LET d = (Distance (3.0))",
        );
        let err = test_run.run_one_err(test_run.parse_one("d.x"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("`x`") && msg.contains("`Number`") && !msg.contains("'s'")),
            "expected a ShapeError naming the field and the operand type, got {err}",
        );
    }

    /// An opaque (`:|`) view re-tags a VAL-slot read with the per-call abstract identity:
    /// `int_ord_view.zero` reads as the abstract `Carrier` (`ktype().name() == "Carrier"`), not the
    /// underlying `Number`, so a deferred return `er.Carrier` accepts the body.
    #[test]
    fn opaque_view_slot_read_re_tags_with_abstract_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))\n\
             MODULE int_ord = ((LET Carrier = Number) (LET zero = 0))\n\
             LET int_ord_view = (int_ord :| WithZero)",
        );
        let types = test_run.registry_handle();
        let result = test_run.run_one(test_run.parse_one("int_ord_view.zero"));
        assert_eq!(
            result.ktype().name(types.registries()),
            "Carrier",
            "opaque-view slot read must carry the abstract `Carrier` identity, got {:?}",
            result.ktype(),
        );
    }

    /// Transparent (`:!`) views leave `slot_type_tags` empty, so the slot read stays
    /// concrete: `int_ord_view.zero` reads as the underlying `Number`, not the abstract `Type`.
    #[test]
    fn transparent_view_slot_read_stays_concrete() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "SIG WithZero = ((TYPE Carrier) (VAL zero :Carrier))\n\
             MODULE int_ord = ((LET Carrier = Number) (LET zero = 0))\n\
             LET int_ord_view = (int_ord :! WithZero)",
        );
        let result = test_run.run_one(test_run.parse_one("int_ord_view.zero"));
        assert!(
            matches!(result, KObject::Number(n) if *n == 0.0),
            "transparent-view slot read must stay the underlying Number, got {:?}",
            result.ktype(),
        );
    }

    /// ATTR on a bare signature type value — not a module/view instance — reads a `VAL` slot's
    /// declared type under the type sigil: `:(Ordered.compare)` yields the slot's declared
    /// `Number`, as a type-side result (a `VAL` slot is a specification, never a value).
    #[test]
    fn sigiled_attr_on_signature_type_reads_val_slot_declared_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG Ordered = (VAL compare :Number)");
        let kt = test_run.run_one_type(test_run.parse_one(":(Ordered.compare)"));
        assert_eq!(kt, KType::NUMBER);
    }

    /// The bare spelling names no member: `compare` is a value token, and a value token never
    /// names a type in an unsigiled expression. The schema does carry the slot, so the miss points
    /// at the surface that reads it.
    #[test]
    fn bare_attr_on_signature_type_misses_with_the_sigil_hint() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG Ordered = (VAL compare :Number)");
        let err = test_run.run_one_err(test_run.parse_one("Ordered.compare"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("signature `Ordered` has no member `compare`")
                    && msg.contains("`:(Ordered.compare)`")),
            "expected the no-member miss hinting the sigiled spelling, got {err}",
        );
    }

    /// A Type-class member is a type name already, so it projects in either context — the sigil
    /// admits a *value* token into the type universe and changes nothing else.
    #[test]
    fn a_type_class_signature_member_projects_in_either_context() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG Ordered = (TYPE Carrier)");
        let bare = test_run.run_one_type(test_run.parse_one("Ordered.Carrier"));
        let sigiled = test_run.run_one_type(test_run.parse_one(":(Ordered.Carrier)"));
        assert_eq!(bare, sigiled);
    }

    /// A field the signature does not carry takes the plain miss: there is no sigiled spelling
    /// that would answer it, so nothing is appended.
    #[test]
    fn a_signature_miss_the_sigil_would_not_rescue_is_unhinted() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("SIG Ordered = (VAL compare :Number)");
        let err = test_run.run_one_err(test_run.parse_one("Ordered.missing"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("has no member `missing`") && !msg.contains("write `:(")),
            "expected the bare no-member miss, got {err}",
        );
    }

    /// `:(Point.x)` — the type sigil over an ATTR with a record-repr-newtype type lhs — yields the
    /// field's declared type off the member's sealed `NodeSchema::NewType` record schema.
    #[test]
    fn sigiled_attr_on_a_record_repr_newtype_reads_the_field_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Str}");
        assert_eq!(
            test_run.run_one_type(test_run.parse_one(":(Point.x)")),
            KType::NUMBER
        );
        assert_eq!(
            test_run.run_one_type(test_run.parse_one(":(Point.y)")),
            KType::STR
        );
    }

    /// The projected type *is* the field's declared type, so a slot spelled `:(Point.y)` admits
    /// exactly what a slot spelled `:Str` admits — a `Str` binds, a `Number` is a dispatch
    /// non-match.
    #[test]
    fn a_slot_typed_by_a_projected_field_admits_what_the_declared_type_admits() {
        let program = program_storage();
        let region = run_root_storage();
        let (mut test_run, captured) = TestRun::with_buf(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Str}\n\
             FN (TAKEY v :(Point.y)) -> Str = (v)\n\
             PRINT (TAKEY \"s\")",
        );
        let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
        assert_eq!(printed, "s\n");

        let err = test_run.run_one_err(test_run.parse_one("TAKEY 3"));
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "a Number is a non-match for a `:Str`-declared slot, got {err}",
        );
    }

    /// A union variant is a Type-class member, so both spellings name it and name the same thing.
    #[test]
    fn a_union_variant_projects_the_same_bare_and_under_the_sigil() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)");
        assert_eq!(
            test_run.run_one_type(test_run.parse_one("Maybe.Some")),
            test_run.run_one_type(test_run.parse_one(":(Maybe.Some)")),
        );
    }

    /// A `LET`-bound alias resolves to the same `KType`, so it enters the projection ladder
    /// identically.
    #[test]
    fn a_let_bound_alias_of_the_nominal_projects_the_same_field_type() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Str}\nLET Pointer = Point");
        assert_eq!(
            test_run.run_one_type(test_run.parse_one(":(Pointer.x)")),
            KType::NUMBER
        );
    }

    /// The sigil marks one node, so the handler re-labels the qualifying lhs on its way down and
    /// `:(Point.inner.x)` resolves both levels.
    #[test]
    fn a_sigiled_projection_chains_through_a_record_shaped_field() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Inner = :{x :Number}\nNEWTYPE Point = :{inner :Inner, tag :Str}");
        assert_eq!(
            test_run.run_one_type(test_run.parse_one(":(Point.inner.x)")),
            KType::NUMBER
        );
    }

    /// Bare `Point.x` names no member, and the schema carries the field, so the miss hints the
    /// sigiled spelling — the same refinement the signature arm makes.
    #[test]
    fn bare_attr_on_a_record_repr_newtype_misses_with_the_sigil_hint() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Str}");
        let err = test_run.run_one_err(test_run.parse_one("Point.x"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("type `Point` has no member `x`")
                    && msg.contains("`:(Point.x)`")),
            "expected the no-member miss hinting the sigiled spelling, got {err}",
        );
    }

    /// A field the record does not carry reports the bare no-member message unhinted in value
    /// context, and lists the record's fields under the sigil.
    #[test]
    fn an_unknown_record_repr_field_lists_the_fields_under_the_sigil() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Point = :{x :Number, y :Str}");
        let bare = test_run.run_one_err(test_run.parse_one("Point.zz"));
        assert!(
            matches!(&bare.kind, KErrorKind::ShapeError(msg)
                if msg.contains("type `Point` has no member `zz`") && !msg.contains("write `:(")),
            "expected the bare no-member miss, got {bare}",
        );
        let sigiled = test_run.run_one_err(test_run.parse_one(":(Point.zz)"));
        assert!(
            matches!(&sigiled.kind, KErrorKind::ShapeError(msg)
                if msg.contains("`zz` is not a member of `Point`") && msg.contains("x, y")),
            "expected the field-list miss under the sigil, got {sigiled}",
        );
    }

    /// The stamp is a fact about the node, so it rides the resplice a park installs: a forward
    /// referenced nominal resolves on wake and the field read still runs in the type universe.
    #[test]
    fn the_sigil_survives_a_park_on_a_forward_referenced_nominal() {
        let program = program_storage();
        let region = run_root_storage();
        let (mut test_run, captured) = TestRun::with_buf(&program, &region);
        test_run.run(
            "MODULE m = (\n  \
               FN (TAKEX v :(Point.x)) -> Number = (v)\n  \
               NEWTYPE Point = :{x :Number, y :Str}\n\
             )\n\
             USING m SCOPE (PRINT (TAKEX 3))",
        );
        let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
        assert_eq!(printed, "3\n");
    }

    /// A chain whose lhs sigiled itself — `m.Wrapper` has a Type-class tail, so `build_attr`
    /// already wrapped it — carries the stamp down the same way a re-labelled part does.
    #[test]
    fn a_module_qualified_nominal_chains_under_the_sigil() {
        let program = program_storage();
        let region = run_root_storage();
        let (mut test_run, captured) = TestRun::with_buf(&program, &region);
        test_run.run(
            "MODULE m = (\n  \
               NEWTYPE Wrapper = :{inner :Point}\n  \
               NEWTYPE Point = :{x :Number}\n\
             )\n\
             USING m SCOPE (\n  \
               FN (TAKEX v :(m.Wrapper.inner.x)) -> Number = (v)\n  \
               PRINT (TAKEX 8)\n\
             )",
        );
        let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
        assert_eq!(printed, "8\n");
    }

    /// The value channel is untouched by the split: an `Identifier` lhs is a runtime value
    /// wherever it is written, so a module member read answers the same bare and inside a sigil,
    /// and neither spelling needs the other's.
    #[test]
    fn a_module_value_read_stays_on_the_value_channel_under_the_sigil() {
        let program = program_storage();
        let region = run_root_storage();
        let (mut test_run, captured) = TestRun::with_buf(&program, &region);
        test_run.run(
            "MODULE int_ord = (LET compare = 5)\n\
             PRINT int_ord.compare\n\
             PRINT (TYPE OF int_ord.compare)\n\
             PRINT (:(TYPE OF int_ord.compare))",
        );
        let printed = String::from_utf8(captured.borrow().clone()).expect("PRINT output is UTF-8");
        assert_eq!(printed, "5\nNumber\nNumber\n");
    }

    /// A scalar-repr newtype has no fields to name, so it takes the memberless-type error in
    /// either context.
    #[test]
    fn a_scalar_repr_newtype_still_reports_no_members() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Meters = Number");
        let err = test_run.run_one_err(test_run.parse_one(":(Meters.x)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("type `Meters` has no member `x`")),
            "expected the memberless-type error, got {err}",
        );
    }

    /// A missing field on the wrapped record names the carrier's nominal type in the
    /// `ShapeError`. The newtype-over-newtype collapse peels the inner `Point` identity, so
    /// `b = Boxed(p)` wraps the bare record tagged `Boxed`; the diagnostic names `Boxed`.
    #[test]
    fn access_field_falls_through_wrapped_with_missing_field() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(
            "NEWTYPE Point = :{x :Number, y :Number}\n\
             NEWTYPE Boxed = Point\n\
             LET p = (Point {x = 1, y = 2})\n\
             LET b = (Boxed (p))",
        );
        let err = test_run.run_one_err(test_run.parse_one("b.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Boxed") && msg.contains("`z`")),
            "expected ShapeError naming Boxed and z on Wrapped fall-through, got {err}",
        );
    }
    /// An anonymous record value is projectable on its own: `person.name` reads the field with no
    /// `NEWTYPE` declaration anywhere in the program.
    #[test]
    fn attr_reads_field_off_anonymous_record() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET person = {name = \"Ada\", age = 36}");
        assert!(
            matches!(test_run.run_one(test_run.parse_one("person.name")), KObject::KString(s) if *s == "Ada"),
        );
        assert!(
            matches!(test_run.run_one(test_run.parse_one("person.age")), KObject::Number(n) if *n == 36.0),
        );
    }

    /// The dynamic spelling tracks the dotted one: `ATTR person "name"` reads the same field off
    /// the same anonymous record.
    #[test]
    fn attr_dynamic_string_reads_anonymous_record() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET person = {name = \"Ada\", age = 36}");
        assert!(
            matches!(test_run.run_one(test_run.parse_one("ATTR person \"name\"")), KObject::KString(s) if *s == "Ada"),
        );
    }

    /// A field name computed at runtime reads the same cell: the name is classified and interned at
    /// the read, so a bound string reaches the same symbol the token spelling does.
    #[test]
    fn attr_computed_field_reads_anonymous_record() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET person = {name = \"Ada\", age = 36}\nLET which = \"name\"");
        assert!(
            matches!(test_run.run_one(test_run.parse_one("ATTR person (which)")), KObject::KString(s) if *s == "Ada"),
        );
    }

    /// A miss on an anonymous record names the field, the same `ShapeError` shape a `NEWTYPE`
    /// record's miss reports — the receiver renders as the structural type it carries, since there
    /// is no nominal layer to name.
    #[test]
    fn attr_missing_field_on_anonymous_record_names_field() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET person = {name = \"Ada\", age = 36}");
        let err = test_run.run_one_err(test_run.parse_one("person.email"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("`email`") && msg.contains("name") && !msg.contains("'s'")),
            "expected ShapeError naming email and the record type, got {err}",
        );
    }

    /// The read composes with the schema-typed parameter path: a body whose parameter is typed
    /// `:{name :Str}` reads the field off a wider anonymous record passed to it.
    #[test]
    fn attr_reads_field_through_schema_typed_param() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("FN (GREET r :{name :Str}) -> Str = (r.name)");
        assert!(
            matches!(test_run.run_one(test_run.parse_one("GREET {name = \"Ada\", age = 36}")), KObject::KString(s) if *s == "Ada"),
        );
    }

    /// A projection binds an anonymous record and reads back through the same arm.
    #[test]
    fn attr_reads_field_off_projection() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET both = {x = 1, y = 2, z = 3}\nLET view = ((x y) FROM both)");
        assert!(
            matches!(test_run.run_one(test_run.parse_one("view.x")), KObject::Number(n) if *n == 1.0),
        );
        // The projection narrows the carried type, which is the surface the read admits: `z` is
        // still physically in the shared substrate and still unreadable through the view.
        let err = test_run.run_one_err(test_run.parse_one("view.z"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`z`")),
            "expected ShapeError naming z on a projected-away field, got {err}",
        );
    }

    /// A type-channel lhs carrying no members at all reports the miss by naming the type and the
    /// member, claiming no argument slot — `Number.foo` is a program a writer can reach.
    #[test]
    fn attr_on_a_memberless_type_names_the_type_and_member() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one("Number.foo"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Number") && msg.contains("`foo`") && !msg.contains("'s'")
                    && !msg.contains("write `:(")),
            "expected ShapeError naming Number and foo, got {err}",
        );
    }

    /// `Maybe.Some` — ATTR with a union-typed lhs — projects the variant's member handle as a type
    /// value. Member projection is the one door to a variant, so it is the surface both a
    /// reference and a construction go through.
    #[test]
    fn attr_projects_a_union_variant_member() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)");
        let kt = test_run.run_one_type(test_run.parse_one("Maybe.Some"));
        assert_eq!(kt.name(test_run.registries()), "Some");
    }

    /// A `LET`-bound alias installs the identical union node, so the alias projects the same
    /// members its binder does — the lhs is read by node, never by the name that reached it.
    #[test]
    fn attr_projects_a_variant_through_a_union_alias() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)\nLET Perhaps = Maybe");
        let kt = test_run.run_one_type(test_run.parse_one("Perhaps.None"));
        assert_eq!(kt.name(test_run.registries()), "None");
    }

    /// An inline `:(A | B)` holds *structural* members, which declare no name of their own. Such a
    /// member answers the field that resolves in the reading scope to its own handle.
    #[test]
    fn attr_projects_a_structural_member_of_an_inline_union() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET NumStr = :(Number | Str)");
        let kt = test_run.run_one_type(test_run.parse_one("NumStr.Number"));
        assert_eq!(kt, KType::NUMBER);
    }

    /// An unknown member lists the union's members, and names the lhs as the source spelled it —
    /// a user `UNION` binds an anonymous union, whose own rendering names nothing that was written.
    #[test]
    fn attr_unknown_union_member_lists_the_members() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)");
        let err = test_run.run_one_err(test_run.parse_one("Maybe.Bogus"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg == "`Bogus` is not a member of `Maybe` (members: Some, None)"),
            "expected the member-list miss naming the lhs as spelled, got {err}",
        );
    }

    /// A Value-class field takes the same miss: a member name is a `Type` token, so the casing
    /// mistake reads off the member list.
    #[test]
    fn attr_value_class_field_on_a_union_takes_the_member_miss() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("UNION Maybe = (Some :Number None :Null)");
        let err = test_run.run_one_err(test_run.parse_one("Maybe.some"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg)
                if msg.contains("`some` is not a member of `Maybe`")
                    && msg.contains("Some, None")
                    && !msg.contains("write `:(")),
            "expected the member-list miss for a value-class field, got {err}",
        );
    }
}
