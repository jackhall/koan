//! The door a component of type binders comes into being through: one call per component, every
//! member read before anything is installed, and a refusal that writes nothing.
//!
//! The type channel's analogue of [the tie](../knot/README.md#the-tie). The value channel's
//! tie and this door are the two ways a component of binders becomes values, and the layer above
//! chooses by the channel its members are declared in. The door hands back handles and takes no
//! writer: minting each member's [`TypeValue`](crate::values::TypeValue) and naming the region it
//! lives in stay the caller's.
//!
//! See [README.md § Declarations](README.md#declarations).

use crate::memory::{BumpAllocator, BumpVec, ScopeId};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::binder::{bounded, symbol_from_quote_body};
use crate::parse::builtin_shapes::role::{DefinitionKind, Role};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{ActivationView, BuiltinGroup, Component, Site, is_equality};
use crate::symbols::{KeywordSymbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredGroup, FoldDirection, KKind, KType, RecursiveGroupWindow, ReductionMode,
    RelativeSchema, SchemaDraft, TypeRegistry,
};
use crate::values::KnottedFamily;

use super::Elaboration;
use super::expression::{Elaborator, Groups};
use super::signature::operator_shape;

/// One `KType` per member of `component`, in member order: a `NEWTYPE`'s newtype over its
/// representation, a `NEWTYPE (Key Val AS Pair)`'s constructor family, a `UNION`'s canonical union
/// over its variants, a `SIG`'s signature, and a `LET` of a type name's type expression.
///
/// Every member is read and every schema elaborated before the group seals, so a member of a
/// sealed group reads its fellows as siblings and a ring of declarations needs no placeholder. A
/// declaration the door cannot elaborate refuses with the same [`Elaboration`] a type expression
/// does, naming the site, and writes nothing: the window, its member list and every staged run
/// live in `scratch`, and the only durable write is the registry intern, which is content-
/// addressed and idempotent.
pub fn type_declarations<'graph, 'x, XF: KnottedFamily<'graph>>(
    component: &Component<'graph>,
    reader: &ActivationView<'graph, '_, XF>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<&'x [KType], Elaboration> {
    let shape = reader.shape();
    let mut read = BumpVec::with_capacity_in(component.members.len(), scratch);
    for slot in component.members {
        let node = shape
            .declarations(*slot)
            .expect("a type binder records its declaration node");
        read.push(Declaration::of(node, scratch)?);
    }

    // Only a nominal member can close a cycle: a transparent alias and a signature name no fresh
    // identity, so a cycle through one has no finite type. The rewrite's restatement of the
    // nominal cut the value channel's tie already makes.
    if let Some(member) = read.iter().find(|member| !member.is_nominal())
        && (component.cyclic || read.len() > 1)
    {
        return Err(Elaboration::Unsupported { site: member.site });
    }
    if let [member] = &*read
        && !member.is_nominal()
    {
        let handle = member.standalone(reader, types, scratch)?;
        return Ok(scratch.alloc_slice_copy(&[handle]));
    }

    // The whole member and binder list is fixed before any schema elaborates, because a schema's
    // fellow mention must already have an index to name.
    let mut members = BumpVec::new_in(scratch);
    let mut binders = BumpVec::new_in(scratch);
    let mut fellows = BumpVec::with_capacity_in(component.members.len(), scratch);
    for (member, slot) in read.iter().zip(component.members) {
        let first = members.len();
        match member.kind {
            Declared::NewType { .. } => {
                members.push((member.name, None, KKind::NewType));
                fellows.push((*slot, types.sibling(first)));
            }
            Declared::Family { .. } => {
                members.push((member.name, None, KKind::TypeConstructor));
                fellows.push((*slot, types.sibling(first)));
            }
            Declared::Union { variants } => {
                for (tag, _) in variants {
                    members.push((*tag, Some(member.name), KKind::NewType));
                }
                let owned: &[usize] = scratch.alloc_slice_fill_iter(first..members.len());
                binders.push((member.name, owned));
                let mut siblings = BumpVec::with_capacity_in(owned.len(), scratch);
                siblings.extend(owned.iter().map(|index| types.sibling(*index)));
                fellows.push((*slot, types.union_of(scratch, &siblings)));
            }
            Declared::Signature(_) | Declared::Alias(_) => {
                unreachable!("a non-nominal member is answered alone above")
            }
        }
    }
    let window = RecursiveGroupWindow::for_component(scratch, &members, &binders);

    let mut sealed = None;
    let mut index = 0;
    for member in read.iter() {
        let elaborator = Elaborator {
            reader,
            types,
            scratch,
            fellows: &fellows,
            locals: &[],
        };
        match member.kind {
            Declared::NewType { repr } => {
                let repr = elaborator.part(repr, &TOP)?;
                sealed = window.fill_member(index, RelativeSchema::NewType(repr), types, scratch);
                index += 1;
            }
            Declared::Family { params } => {
                let schema = RelativeSchema::constructor(scratch, scratch, &[], params);
                sealed = window.fill_member(index, schema, types, scratch);
                index += 1;
            }
            Declared::Union { variants } => {
                for (_, payload) in variants {
                    let payload = elaborator.part(payload, &TOP)?;
                    sealed =
                        window.fill_member(index, RelativeSchema::NewType(payload), types, scratch);
                    index += 1;
                }
            }
            Declared::Signature(_) | Declared::Alias(_) => unreachable!("answered alone above"),
        }
    }
    let sealed = sealed.expect("the last fill seals a window whose every member was filled");

    let mut handles = BumpVec::with_capacity_in(read.len(), scratch);
    let mut index = 0;
    for member in read.iter() {
        let handle = match member.kind {
            Declared::NewType { .. } | Declared::Family { .. } => {
                let handle = sealed.member(index).expect("a filled member seals");
                index += 1;
                handle
            }
            Declared::Union { variants } => {
                index += variants.len();
                sealed
                    .binder_type(member.name)
                    .expect("a declaring binder seals its union")
            }
            Declared::Signature(_) | Declared::Alias(_) => unreachable!("answered alone above"),
        };
        handles.push(handle);
    }
    Ok(scratch.alloc_slice_copy(&handles))
}

/// The group-free top of the quantifier stack: a declaration's own part encloses no `FOR ALL`.
const TOP: Groups<'static> = Groups {
    names: &[],
    bounds: &[],
    outer: None,
};

/// One member of a component, read off its declaration node before anything is elaborated.
struct Declaration<'graph, 'x> {
    /// The declared name — the binder's, never a variant tag's.
    name: TypeSymbol,
    /// Where a refusal about this member points: its declaration's leading keyword.
    site: Site,
    kind: Declared<'graph, 'x>,
}

/// What a member's declaration node says it is.
#[derive(Clone, Copy)]
enum Declared<'graph, 'x> {
    /// `NEWTYPE <Name> = <repr>`: one window member over its representation.
    NewType {
        repr: &'graph ExpressionPart<'graph>,
    },
    /// `UNION <Name> = (<Tag> :<payload> …)`: one window member per variant, the binder itself
    /// denoting their union.
    Union {
        variants: &'x [(TypeSymbol, &'graph ExpressionPart<'graph>)],
    },
    /// `NEWTYPE (<P>… AS <Name>)`: one window member, an empty-schema constructor family over its
    /// declared parameter names — the identity wrapper over its argument, so a constructor
    /// application has a declared referent a koan program can write.
    Family { params: &'x [TypeSymbol] },
    /// `SIG <Name> = <body>`: a signature, which no `Sibling` can stand for and so takes part in
    /// no cycle.
    Signature(&'graph ExpressionPart<'graph>),
    /// `LET <Type> = <rhs>`: a transparent alias of its right-hand side.
    Alias(&'graph ExpressionPart<'graph>),
}

impl<'graph, 'x> Declaration<'graph, 'x> {
    /// Read one declaration node: which declaration it is, and where its declared part sits, from
    /// its builtin shape.
    fn of(
        node: &'graph KExpression<'graph>,
        scratch: BumpAllocator<'x>,
    ) -> Result<Self, Elaboration> {
        let site = Site::of(&node.parts[0].value);
        let unsupported = Elaboration::Unsupported { site };
        let form = node.cache().builtin_shape().ok_or(unsupported)?;
        let mut name_part = None;
        let mut declared = None;
        for (role, part) in form.roles().zip(node.parts) {
            match role {
                Role::Name => name_part = Some(&part.value),
                Role::Definition(_) | Role::Rhs => declared = Some(&part.value),
                _ => {}
            }
        }
        let name_part = name_part.ok_or(unsupported)?;
        // A constructor family declares no bound.
        if form.id == BuiltinShapeId::NewTypeDeclaration && bounded(name_part).is_some() {
            return Err(unsupported);
        }
        let kind = match form.id {
            BuiltinShapeId::NewTypeDefinition => Declared::NewType {
                repr: declared.ok_or(unsupported)?,
            },
            BuiltinShapeId::Union => Declared::Union {
                variants: variants(declared.ok_or(unsupported)?, scratch).ok_or(unsupported)?,
            },
            BuiltinShapeId::NewTypeDeclaration => Declared::Family {
                params: parameters(name_part, scratch).ok_or(unsupported)?,
            },
            BuiltinShapeId::Sig => Declared::Signature(declared.ok_or(unsupported)?),
            BuiltinShapeId::LetValue => Declared::Alias(declared.ok_or(unsupported)?),
            // A bare `TYPE` names an abstract member only a signature can bind.
            _ => return Err(unsupported),
        };
        let name = match form.id {
            BuiltinShapeId::NewTypeDeclaration => last_type_name(name_part),
            _ => match name_part {
                ExpressionPart::Type(name) => Some(*name),
                _ => None,
            },
        };
        Ok(Declaration {
            name: name.ok_or(unsupported)?,
            site,
            kind,
        })
    }

    /// Whether this member can be a window member, and so can close a cycle.
    fn is_nominal(&self) -> bool {
        matches!(
            self.kind,
            Declared::NewType { .. } | Declared::Union { .. } | Declared::Family { .. }
        )
    }

    /// The handle of a member declared alone, outside any window.
    fn standalone<XF: KnottedFamily<'graph>>(
        &self,
        reader: &ActivationView<'graph, '_, XF>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<KType, Elaboration> {
        let elaborator = Elaborator {
            reader,
            types,
            scratch,
            fellows: &[],
            locals: &[],
        };
        match self.kind {
            Declared::Alias(rhs) => elaborator.part(rhs, &TOP),
            Declared::Signature(body) => signature_type(&elaborator, body, self.site),
            _ => unreachable!("a nominal member seals in a window"),
        }
    }
}

/// A `UNION`'s `(<Tag> :<payload> …)` run as tag/payload pairs, in written order: the tags sit at
/// the even indices, exactly as the shape builder skips them.
fn variants<'graph, 'x>(
    part: &'graph ExpressionPart<'graph>,
    scratch: BumpAllocator<'x>,
) -> Option<&'x [(TypeSymbol, &'graph ExpressionPart<'graph>)]> {
    let ExpressionPart::Expression(run) = part else {
        return None;
    };
    let run = run.reference();
    if run.parts.is_empty() || !run.parts.len().is_multiple_of(2) {
        return None;
    }
    let mut pairs = BumpVec::with_capacity_in(run.parts.len() / 2, scratch);
    for pair in run.parts.chunks_exact(2) {
        let ExpressionPart::Type(tag) = pair[0].value else {
            return None;
        };
        if pairs.iter().any(|(seen, _)| *seen == tag) {
            return None;
        }
        pairs.push((tag, &pair[1].value));
    }
    Some(pairs.leak())
}

/// The parameter names of a `(<P>… AS <Name>)` declarator, in written order: every `Type` part
/// before the trailing name. A repeated name is refused.
fn parameters<'x>(
    part: &ExpressionPart<'_>,
    scratch: BumpAllocator<'x>,
) -> Option<&'x [TypeSymbol]> {
    let ExpressionPart::Expression(run) = part else {
        return None;
    };
    let run = run.reference();
    let (_, declared) = run.parts.split_last()?;
    let mut names = BumpVec::with_capacity_in(declared.len(), scratch);
    for part in declared {
        let ExpressionPart::Type(name) = part.value else {
            continue;
        };
        if names.contains(&name) {
            return None;
        }
        names.push(name);
    }
    (!names.is_empty()).then(|| &*names.leak())
}

/// The name a `(<P>… AS <Name>)` declarator binds: the run's last `Type` part.
fn last_type_name(part: &ExpressionPart<'_>) -> Option<TypeSymbol> {
    let ExpressionPart::Expression(run) = part else {
        return None;
    };
    match run.reference().parts.last()?.value {
        ExpressionPart::Type(name) => Some(name),
        _ => None,
    }
}

/// A `SIG`'s signature over its body's declarators, in source order.
///
/// Its abstract and manifest members are the body's own names, resolved through the elaborator's
/// local table rather than through a mention, since the shape declared them in the definition. A
/// bodyless `EXPR`, `OP` or `UNARY OP` head is a keyworded member; a bodyless `GROUP` is the
/// operator channel's and is refused here.
fn signature_type<'graph, XF: KnottedFamily<'graph>>(
    elaborator: &Elaborator<'_, '_, 'graph, '_, '_, XF>,
    body: &'graph ExpressionPart<'graph>,
    site: Site,
) -> Result<KType, Elaboration> {
    let unsupported = Elaboration::Unsupported { site };
    let ExpressionPart::Expression(body) = body else {
        return Err(unsupported);
    };
    let scratch = elaborator.scratch;
    let types = elaborator.types;
    let mut draft = SchemaDraft::new(scratch);
    // A textually identical `SIG` in another program is one type: the sentinel is stamped here
    // rather than round-tripped through the declaring scope's own id.
    draft.sig_id = Some(ScopeId::SENTINEL);
    let mut locals: BumpVec<'_, (TypeSymbol, KType)> = BumpVec::new_in(scratch);
    // The groups this signature declares, and the symbols of the heads that state a result of
    // their own — admitted only where the symbol chains pairwise, which a later group may settle.
    let mut groups: BumpVec<'_, DeclaredGroup<'_>> = BumpVec::new_in(scratch);
    let mut returning: BumpVec<'_, (KeywordSymbol, Site)> = BumpVec::new_in(scratch);

    for (statement, _) in body.reference().body_statements() {
        let node = statement.statement_spine();
        let member = Elaborator {
            reader: elaborator.reader,
            types,
            scratch,
            fellows: elaborator.fellows,
            locals: &locals,
        };
        let site = Site::of(&node.parts[0].value);
        let unsupported = Elaboration::Unsupported { site };
        let form = node.cache().builtin_shape().ok_or(unsupported)?;
        let mut name_part = None;
        let mut label = None;
        let mut rhs = None;
        let mut data = None;
        let mut argument = None;
        let mut definition = None;
        let mut type_parts = [None; 2];
        let mut type_count = 0;
        for (role, part) in form.roles().zip(node.parts) {
            match role {
                Role::Name => name_part = Some(&part.value),
                Role::Label => label = Some(&part.value),
                Role::Rhs => rhs = Some(&part.value),
                Role::Data => data = Some(&part.value),
                Role::Argument => argument = Some(&part.value),
                Role::Definition(DefinitionKind::Plain) => definition = Some(&part.value),
                Role::TypeExpression => {
                    type_parts[type_count] = Some(&part.value);
                    type_count += 1;
                }
                _ => {}
            }
        }
        match form.id {
            BuiltinShapeId::TypeDeclaration => {
                let name_part = name_part.ok_or(unsupported)?;
                let (declarator, bound) = match bounded(name_part) {
                    Some((declarator, bound)) => (declarator, Some(bound)),
                    None => (name_part, None),
                };
                let (name, params) = match declarator {
                    ExpressionPart::Type(name) => (*name, &[][..]),
                    _ => (
                        last_type_name(declarator).ok_or(unsupported)?,
                        parameters(declarator, scratch).ok_or(unsupported)?,
                    ),
                };
                // A bound reads earlier members through `locals`, so one naming an abstract member
                // is refused and one naming a manifest member reads its type. A higher-kinded
                // member takes no bound.
                let bound = match bound {
                    Some(_) if !params.is_empty() => return Err(unsupported),
                    Some(part) => member.bound(part, &TOP)?,
                    None => KType::ANY,
                };
                let handle =
                    types.abstract_type(scratch, ScopeId::SENTINEL, name, params, None, bound);
                draft.insert_abstract(name, handle);
                locals.push((name, handle));
            }
            BuiltinShapeId::LetValue => {
                let ExpressionPart::Type(name) = name_part.ok_or(unsupported)? else {
                    return Err(unsupported);
                };
                let handle = member.part(rhs.ok_or(unsupported)?, &TOP)?;
                draft.insert_manifest(*name, handle);
                locals.push((*name, handle));
            }
            BuiltinShapeId::Val => {
                let ExpressionPart::Identifier(name) = label.ok_or(unsupported)? else {
                    return Err(unsupported);
                };
                let handle = member.part(type_parts[0].ok_or(unsupported)?, &TOP)?;
                draft.insert_value_slot(*name, handle);
            }
            BuiltinShapeId::ExpressionHead | BuiltinShapeId::QuantifiedExpressionHead => {
                draft.push_keyworded(member.node(site, node, &TOP)?);
            }
            BuiltinShapeId::OperatorHead
            | BuiltinShapeId::OperatorHeadReturning
            | BuiltinShapeId::UnaryOperatorHeadReturning => {
                let data = data.ok_or(unsupported)?;
                let shape = operator_shape(
                    &member,
                    form.id == BuiltinShapeId::UnaryOperatorHeadReturning,
                    data,
                    type_parts[0].ok_or(unsupported)?,
                    type_parts[1],
                    &TOP,
                )?;
                if form.id == BuiltinShapeId::OperatorHeadReturning {
                    returning.push((quoted_operator(data).ok_or(unsupported)?, site));
                }
                draft.push_keyworded(shape);
            }
            // A bodyless `GROUP` declares a chaining record, which the operator channel owns.
            BuiltinShapeId::GroupHeadFoldLeft
            | BuiltinShapeId::GroupHeadFoldRight
            | BuiltinShapeId::GroupHeadPairwiseFoldLeft
            | BuiltinShapeId::GroupHeadPairwiseFoldRight => {
                let mode = group_mode(form.id, argument).ok_or(unsupported)?;
                let members = group_members(
                    &member,
                    definition.ok_or(unsupported)?,
                    mode,
                    &mut draft,
                    scratch,
                )?;
                let declared = DeclaredGroup {
                    members: &members,
                    mode,
                };
                // A symbol chains one way. A second group of this signature over a member, and a
                // member of a builtin group this one is not, would each give it a second.
                for symbol in &members {
                    if groups
                        .iter()
                        .any(|group: &DeclaredGroup<'_>| group.members.contains(symbol))
                    {
                        return Err(unsupported);
                    }
                    if BuiltinGroup::of(*symbol).is_some_and(|builtin| !builtin.equals(&declared)) {
                        return Err(unsupported);
                    }
                }
                groups.push(DeclaredGroup {
                    members: scratch.alloc_slice_copy(&members),
                    mode,
                });
                draft.push_operator_group(&members, mode);
            }
            _ => return Err(unsupported),
        }
    }
    // A binary head states a result of its own only where its symbol chains pairwise — a fold
    // carries its operand type forward, so a result that is not the operand has nowhere to go.
    for (symbol, site) in &returning {
        let pairwise = |mode| matches!(mode, ReductionMode::Pairwise { .. });
        let chains = is_equality(*symbol)
            || BuiltinGroup::of(*symbol).is_some_and(|group| pairwise(group.mode()))
            || groups
                .iter()
                .any(|group| group.members.contains(symbol) && pairwise(group.mode))
            || elaborator.reader.shape().group_frame().pairwise(*symbol);
        if !chains {
            return Err(Elaboration::Unsupported { site: *site });
        }
    }
    Ok(types.signature(scratch, draft))
}

/// How a run of a `SIG` body's bodyless `GROUP` head reduces, read off its form id and — for a
/// pairwise head — the combiner it quotes.
fn group_mode(
    form: BuiltinShapeId,
    argument: Option<&ExpressionPart<'_>>,
) -> Option<ReductionMode> {
    match form {
        BuiltinShapeId::GroupHeadFoldLeft => Some(ReductionMode::FoldLeft),
        BuiltinShapeId::GroupHeadFoldRight => Some(ReductionMode::FoldRight),
        BuiltinShapeId::GroupHeadPairwiseFoldLeft | BuiltinShapeId::GroupHeadPairwiseFoldRight => {
            Some(ReductionMode::Pairwise {
                combiner: quoted_operator(argument?)?,
                direction: match form {
                    BuiltinShapeId::GroupHeadPairwiseFoldLeft => FoldDirection::Left,
                    _ => FoldDirection::Right,
                },
            })
        }
        _ => None,
    }
}

/// The members a bodyless `GROUP` head declares, each head's shape pushed as a keyworded member as
/// it is read. A group's body is binary operator heads and nothing else, and a head stating a
/// result of its own belongs only to a pairwise group.
fn group_members<'graph, 'x, XF: KnottedFamily<'graph>>(
    elaborator: &Elaborator<'_, '_, 'graph, '_, '_, XF>,
    definition: &'graph ExpressionPart<'graph>,
    mode: ReductionMode,
    draft: &mut SchemaDraft<'x>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, KeywordSymbol>, Elaboration> {
    let unsupported = Elaboration::Unsupported {
        site: Site::of(definition),
    };
    let ExpressionPart::Expression(body) = definition else {
        return Err(unsupported);
    };
    let mut members: BumpVec<'x, KeywordSymbol> = BumpVec::new_in(scratch);
    for (statement, _) in body.reference().body_statements() {
        let head = statement.statement_spine();
        let form = head.cache().builtin_shape().ok_or(unsupported)?;
        let returns = match form.id {
            BuiltinShapeId::OperatorHead => false,
            BuiltinShapeId::OperatorHeadReturning => true,
            _ => return Err(unsupported),
        };
        if returns && !matches!(mode, ReductionMode::Pairwise { .. }) {
            return Err(unsupported);
        }
        let mut data = None;
        let mut type_parts = [None; 2];
        let mut type_count = 0;
        for (role, part) in form.roles().zip(head.parts) {
            match role {
                Role::Data => data = Some(&part.value),
                Role::TypeExpression => {
                    type_parts[type_count] = Some(&part.value);
                    type_count += 1;
                }
                _ => {}
            }
        }
        let data = data.ok_or(unsupported)?;
        let shape = operator_shape(
            elaborator,
            false,
            data,
            type_parts[0].ok_or(unsupported)?,
            type_parts[1],
            &TOP,
        )?;
        draft.push_keyworded(shape);
        let symbol = quoted_operator(data).ok_or(unsupported)?;
        if is_equality(symbol) {
            // `==` and `!=` belong to no group: they join whichever pairwise group the rest of an
            // operator run chains under.
            return Err(unsupported);
        }
        members.push(symbol);
    }
    if members.is_empty() {
        return Err(unsupported);
    }
    members.sort_unstable();
    members.dedup();
    Ok(members)
}

/// The one operator symbol a `#(…)` part quotes.
fn quoted_operator(part: &ExpressionPart<'_>) -> Option<KeywordSymbol> {
    let ExpressionPart::QuotedExpression(quoted) = part else {
        return None;
    };
    symbol_from_quote_body(quoted.reference()).ok()
}
