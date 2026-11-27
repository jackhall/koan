//! The door a component of type binders comes into being through: one call per component, every
//! member read before anything is installed, and a refusal that writes nothing.
//!
//! The type channel's analogue of [the tie](../knot/README.md#the-tie). The value channel's
//! tie and this door are the two ways a component of binders becomes values, and the layer above
//! chooses by the channel its members are declared in. The door hands back handles and takes no
//! writer: minting each member's [`TypeValue`](crate::values::TypeValue) and naming the region it
//! lives in stay the caller's.
//!
//! Only a nominal member opens a window index. A signature or alias in a component that also holds
//! a nominal member is read relative, over its fellows' relative handles, and handed back as the
//! sealed group's absolute twin of that type ([README.md § One window per
//! component](README.md#one-window-per-component)).
//!
//! See [README.md § Declarations](README.md#declarations).

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, collect};
use crate::parse::BuiltinShapeId;
use crate::parse::{DefinitionKind, Role};
use crate::parse::{ExpressionPart, KExpression};
use crate::parse::{
    bounded, declarator_parameters, family_head, quoted_body, quoted_part, symbol_from_quote_body,
};
use crate::scope::{
    BodyShape, BuiltinGroup, Component, DerivedTerm, Elaboration, Handed, Nominals, Schema,
    Signing, Site, Slot, Which, handed, is_equality, sealed_nominals, sealed_signature,
};
use crate::symbols::{KeywordSymbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredGroup, DeclaredType, FoldDirection, KKind, KType, Parametric, ReductionMode,
};

use super::expression::{Elaborator, Fellow, Fellows, Groups};
use super::reads::Reads;
use super::signature::operator_shape;
use crate::values::Chain;

/// Whether the declaration node `node` declares a nominal type — a `NEWTYPE` of either spelling
/// or a `UNION` — the member kinds that name a fresh identity, and so the ones a body declaring
/// them where they run mints.
pub fn declares_nominal(node: &KExpression<'_>) -> bool {
    node.cache().builtin_shape().is_some_and(|form| {
        matches!(
            form.id,
            BuiltinShapeId::NewTypeDefinition
                | BuiltinShapeId::NewTypeDeclaration
                | BuiltinShapeId::Union
        )
    })
}

/// One type per member of `component`, in member order: a `NEWTYPE`'s newtype over its
/// representation, a `NEWTYPE (Key Val -> Pair)`'s constructor family, a `UNION`'s canonical union
/// over its variants — each variant a family over the union's parameters when its declarator names
/// any — a `SIG`'s signature, and a `LET` of a type name's type expression.
///
/// A lone alias reading a name a run binds is rigid as written, over that name's lexical variable.
/// A nominal component reading one is rigid too: each member is the derived name of the group a
/// binding seals ([`Nominals`]), through the one closed rule a closed group seals by
/// ([`sealed_nominals`]), and so is a lone signature ([`sealed_signature`]). A signature or alias
/// fellow of a nominal reading one is rigid alike, sealed with the group.
///
/// Every member is read and every schema elaborated before the group seals, so a member of a
/// sealed group reads its fellows as siblings and a ring of declarations needs no placeholder. Only
/// the nominal members open the window: a signature or alias fellow is read relative where a member
/// names it ([`Relatives`]) and handed back as the sealed group's absolute twin of that relative
/// type. A cycle through signatures and aliases alone has no finite type and is refused at the
/// member it re-enters, or at the first-written member of a component with no nominal one. A
/// declaration the door cannot elaborate refuses with the same [`Elaboration`] a type expression
/// does, naming the site, and writes nothing: the window, its member list and every staged run
/// live in `scratch`, and the only durable write is the registry intern, which is content-
/// addressed and idempotent.
pub fn type_declarations<'graph, 'x, R: Reads<'graph> + ?Sized>(
    component: &Component<'graph>,
    reader: &R,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<&'x [Parametric], Elaboration> {
    let shape = reader.shape();
    let mut read = BumpVec::with_capacity_in(component.members.len(), scratch);
    for slot in component.members {
        let node = shape
            .declarations(*slot)
            .expect("a type binder records its declaration node");
        read.push(Declaration::of(shape, node, scratch)?);
    }

    // Only a nominal member can close a cycle: a transparent alias and a signature name no fresh
    // identity, so a cycle through such members alone has no finite type. The type channel's
    // restatement of the nominal cut the value channel's tie already makes.
    if !read.iter().any(Declaration::is_nominal) {
        if let Some(first) = read.iter().min_by_key(|member| member.site)
            && component.cyclic
        {
            return Err(first.cycle());
        }
        let elaborator = Elaborator {
            reader,
            types,
            scratch,
            fellows: Fellows::NONE,
            locals: &[],
            binder: Cell::new(false),
        };
        let [member] = &*read else {
            unreachable!("an acyclic component is one member")
        };
        return Ok(scratch.alloc_slice_copy(&[member.elaborate(&elaborator)?]));
    }

    // The whole member and binder list is fixed before any schema elaborates, because a schema's
    // fellow mention must already have an index to name. A signature or alias takes no index.
    let mut members = BumpVec::new_in(scratch);
    let mut binders = BumpVec::new_in(scratch);
    let mut fellows = BumpVec::with_capacity_in(component.members.len(), scratch);
    for (member, slot) in read.iter().zip(component.members) {
        let first = members.len();
        match member.kind {
            Declared::NewType { .. } => {
                members.push((member.name, None, KKind::NewType));
                fellows.push(Fellow {
                    slot: *slot,
                    handle: types.sibling(first).into(),
                    params: &[],
                });
            }
            Declared::Family { params } => {
                members.push((member.name, None, KKind::TypeConstructor));
                fellows.push(Fellow {
                    slot: *slot,
                    handle: types.sibling(first).into(),
                    params,
                });
            }
            Declared::Union { variants, params } => {
                let kind = if params.is_empty() {
                    KKind::NewType
                } else {
                    KKind::TypeConstructor
                };
                for (tag, _) in variants {
                    members.push((*tag, Some(member.name), kind));
                }
                let owned: &[usize] = scratch.alloc_slice_fill_iter(first..members.len());
                binders.push((member.name, owned));
                let mut siblings = BumpVec::with_capacity_in(owned.len(), scratch);
                siblings.extend(owned.iter().map(|index| types.sibling(*index)));
                fellows.push(Fellow {
                    slot: *slot,
                    handle: types.union_of(scratch, &siblings).into(),
                    params,
                });
            }
            Declared::Signature { .. } | Declared::Alias(_) => {}
        }
    }

    // A run-bound name read from here on, by a relative or a schema, leaves the group to a binding.
    let rigid = reader.rigid_reads();
    // Each signature and alias, relative: over its nominal fellows' handles, and over each other
    // signature or alias it names, read first. They are read in written order — a component's
    // members are in slot order — so a cycle among them refuses at the first one written.
    let mut relatives = BumpVec::with_capacity_in(read.len(), scratch);
    relatives.resize(read.len(), None);
    {
        let state = scratch.alloc_slice_fill_with(read.len(), |_| Cell::new(Reading::Unread));
        let reading = Relatives {
            read: &read,
            slots: component.members,
            nominal: &fellows,
            state,
            reader,
            types,
            scratch,
        };
        let mut written: BumpVec<'_, usize> = BumpVec::with_capacity_in(read.len(), scratch);
        written.extend((0..read.len()).filter(|index| !read[*index].is_nominal()));
        written.sort_unstable_by_key(|index| read[*index].site);
        for index in written {
            relatives[index] = Some(reading.relative(index)?);
        }
    }
    for (relative, slot) in relatives.iter().zip(component.members) {
        if let Some(handle) = relative {
            fellows.push(Fellow {
                slot: *slot,
                handle: *handle,
                params: &[],
            });
        }
    }

    // Each window member's schema, every type it reads an operand: a newtype's representation, a
    // family's over its parameters' quantifiers.
    let mut operands = BumpVec::new_in(scratch);
    let mut schemas = BumpVec::with_capacity_in(members.len(), scratch);
    for member in read.iter() {
        let elaborator = Elaborator {
            reader,
            types,
            scratch,
            fellows: Fellows {
                read: &fellows,
                unread: None,
            },
            locals: &[],
            binder: Cell::new(false),
        };
        let mut operand = |ty: Parametric| {
            operands.push(ty);
            (operands.len() - 1) as u16
        };
        match member.kind {
            Declared::NewType { repr } => {
                schemas.push(Schema::NewType(operand(elaborator.part(repr, &TOP)?)));
            }
            // A one-parameter family wraps a payload of its parameter's type; a wider one is
            // type-position only.
            Declared::Family { params } => {
                let representation = (params.len() == 1).then(|| types.quantified(0, KType::ANY));
                schemas.push(Schema::Constructor {
                    representation: representation.map(operand),
                    params,
                });
            }
            Declared::Union {
                variants,
                params: [],
            } => {
                for (_, payload) in variants {
                    schemas.push(Schema::NewType(operand(elaborator.part(payload, &TOP)?)));
                }
            }
            // Each variant is a family over all of the union's parameters, its payload read with
            // them as the innermost group. An application is covariant in its arguments, so a
            // parameter at a contravariant position or inside a pin, which is invariant, is refused.
            Declared::Union { variants, params } => {
                let family = Groups {
                    names: params,
                    bounds: scratch.alloc_slice_fill_copy(params.len(), KType::ANY),
                    outer: None,
                };
                for (_, payload) in variants {
                    let representation = elaborator.part(payload, &family)?;
                    if types.quantifies_noncovariantly(scratch, representation, params.len()) {
                        return Err(Elaboration::Unsupported {
                            site: elaborator.reader.shape().site(payload),
                        });
                    }
                    elaborator.untied(
                        elaborator.reader.shape().site(payload),
                        (family.names, family.bounds),
                        [representation],
                    )?;
                    schemas.push(Schema::Constructor {
                        representation: Some(operand(representation)),
                        params,
                    });
                }
            }
            Declared::Signature { .. } | Declared::Alias(_) => {}
        }
    }

    // How each component member's handle is read off the sealed group.
    let mut handles = BumpVec::with_capacity_in(read.len(), scratch);
    let mut index = 0;
    for (member, relative) in read.iter().zip(&relatives) {
        handles.push(match member.kind {
            Declared::NewType { .. } | Declared::Family { .. } => {
                index += 1;
                Handed::Member(index - 1)
            }
            Declared::Union { variants, .. } => {
                index += variants.len();
                Handed::Binder(member.name)
            }
            Declared::Signature { .. } | Declared::Alias(_) => {
                let relative = relative.expect("a signature or alias is read relative");
                operands.push(relative);
                Handed::Relative((operands.len() - 1) as u16)
            }
        });
    }
    let nominals = Nominals {
        members: &members,
        binders: &binders,
        schemas: &schemas,
        handles: &handles,
    };
    // Over a run-bound name, each member is the derived name of the group a binding seals.
    if reader.rigid_reads() > rigid {
        let site = read
            .iter()
            .map(|member| member.site)
            .min()
            .expect("a member");
        let mut named = BumpVec::with_capacity_in(handles.len(), scratch);
        for output in 0..handles.len() {
            let term = DerivedTerm::Nominal {
                group: &nominals,
                operands: &operands,
                output: output as u8,
            };
            let derive = &mut |term: DerivedTerm<'_>| reader.marks().derive(term);
            let handle =
                handed(types, scratch, term, derive).map_err(|unwalked| Elaboration::Unknown {
                    site,
                    unwalked: unwalked.into(),
                })?;
            named.push(handle.expect("a window whose every member a closed schema filled seals"));
        }
        return Ok(named.leak());
    }
    let sealed = sealed_nominals(types, scratch, &nominals, &operands)
        .expect("a window whose every member a closed schema filled seals");
    Ok(collect(
        scratch,
        sealed.iter().map(|handle| Parametric::from(*handle)),
    ))
}

/// How far a component's signature or alias member has been read.
#[derive(Clone, Copy)]
enum Reading {
    Unread,
    /// Being read: a mention reaching it again closes a cycle through signatures and aliases alone.
    Open,
    Read(Parametric),
}

/// A component's signature and alias members, each read relative on first mention: over its
/// nominal fellows' relative handles, and over each other signature or alias it names, read first.
/// A mention that re-enters a member still being read is refused at that member.
struct Relatives<'a, 'graph, 'x, 'tier, 'pair, 'reg, 'door, R: ?Sized> {
    read: &'a [Declaration<'graph, 'x>],
    /// Each member's slot, parallel to `read`.
    slots: &'graph [Slot],
    /// The nominal fellows, as announced.
    nominal: &'a [Fellow<'a>],
    /// Each member's reading, parallel to `read`; a nominal member's stays `Unread`.
    state: &'a [Cell<Reading>],
    reader: &'a R,
    types: &'a Chain<'tier, 'pair, 'reg, 'door>,
    scratch: BumpAllocator<'x>,
}

impl<'graph, R: Reads<'graph> + ?Sized> Relatives<'_, 'graph, '_, '_, '_, '_, '_, R> {
    /// Member `index`'s relative type, read on first call. A call that re-enters a member still
    /// being read is a cycle through signatures and aliases alone, refused at that member.
    fn relative(&self, index: usize) -> Result<Parametric, Elaboration> {
        match self.state[index].get() {
            Reading::Read(handle) => return Ok(handle),
            Reading::Open => return Err(self.read[index].cycle()),
            Reading::Unread => {}
        }
        self.state[index].set(Reading::Open);
        let unread = |slot: Slot| {
            let at = self.slots.iter().position(|member| *member == slot)?;
            (!self.read[at].is_nominal()).then(|| self.relative(at))
        };
        let elaborator = Elaborator {
            reader: self.reader,
            types: self.types,
            scratch: self.scratch,
            fellows: Fellows {
                read: self.nominal,
                unread: Some(&unread),
            },
            locals: &[],
            binder: Cell::new(false),
        };
        // A fellow over a run-bound name is rigid, sealed with the nominals it reads.
        let handle = self.read[index].elaborate(&elaborator)?;
        self.state[index].set(Reading::Read(handle));
        Ok(handle)
    }
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
    /// `UNION <Name> = #{<Tag>: <payload>, …}`: one window member per variant, the binder itself
    /// denoting their union. `UNION (<P>… -> <Name>) = …` declares `params`, symbol-sorted, and
    /// makes each variant a family over them; a bare name declares none.
    Union {
        variants: &'x [(TypeSymbol, &'graph ExpressionPart<'graph>)],
        params: &'x [TypeSymbol],
    },
    /// `NEWTYPE (<P>… -> <Name>)`: one window member, a constructor family over its declared
    /// parameter names, symbol-sorted — the identity wrapper over its argument, which a
    /// construction builds only when there is one parameter.
    Family { params: &'x [TypeSymbol] },
    /// `SIG <Name> [FOR ALL <names>] = <body>`: a signature over its head group, if it writes one.
    /// No window member: it mints no identity, so no `Sibling` stands for it, and inside a
    /// component it is read relative.
    Signature {
        group: Option<&'graph ExpressionPart<'graph>>,
        body: &'graph ExpressionPart<'graph>,
    },
    /// `LET <Type> = <rhs>`: a transparent alias of its right-hand side.
    Alias(&'graph ExpressionPart<'graph>),
}

impl<'graph, 'x> Declaration<'graph, 'x> {
    /// Read one declaration node: which declaration it is, and where its declared part sits, from
    /// its builtin shape.
    fn of(
        shape: &BodyShape<'graph>,
        node: &'graph KExpression<'graph>,
        scratch: BumpAllocator<'x>,
    ) -> Result<Self, Elaboration> {
        let site = shape.site(&node.parts[0].value);
        let unsupported = Elaboration::Unsupported { site };
        let form = node.cache().builtin_shape().ok_or(unsupported)?;
        let mut name_part = None;
        let mut declared = None;
        let mut group = None;
        for (role, part) in form.roles().zip(node.parts) {
            match role {
                Role::Name => name_part = Some(&part.value),
                Role::Definition(_) | Role::Rhs => declared = Some(&part.value),
                Role::Quantifiers => group = Some(&part.value),
                _ => {}
            }
        }
        let name_part = name_part.ok_or(unsupported)?;
        // A constructor family declares no bound, and neither does a parameterized union.
        if matches!(
            form.id,
            BuiltinShapeId::NewTypeDeclaration | BuiltinShapeId::Union
        ) && bounded(name_part).is_some()
        {
            return Err(unsupported);
        }
        let declarator = matches!(name_part, ExpressionPart::Expression(_));
        let kind = match form.id {
            BuiltinShapeId::NewTypeDefinition => Declared::NewType {
                repr: declared.ok_or(unsupported)?,
            },
            BuiltinShapeId::Union => Declared::Union {
                variants: variants(declared.ok_or(unsupported)?, scratch).ok_or(unsupported)?,
                params: if declarator {
                    parameters(name_part, scratch).ok_or(unsupported)?
                } else {
                    &[]
                },
            },
            BuiltinShapeId::NewTypeDeclaration => Declared::Family {
                params: parameters(name_part, scratch).ok_or(unsupported)?,
            },
            BuiltinShapeId::Sig | BuiltinShapeId::QuantifiedSig => Declared::Signature {
                group,
                body: declared.ok_or(unsupported)?,
            },
            BuiltinShapeId::LetValue => Declared::Alias(declared.ok_or(unsupported)?),
            _ => return Err(unsupported),
        };
        let name = match form.id {
            BuiltinShapeId::NewTypeDeclaration | BuiltinShapeId::Union if declarator => {
                family_head(name_part).map(|(_, name)| name)
            }
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

    /// The refusal of a cycle through signatures and aliases alone, at this member.
    fn cycle(&self) -> Elaboration {
        Elaboration::CycleWithoutNominal {
            name: self.name,
            site: self.site,
        }
    }

    /// Whether this member can be a window member, and so can close a cycle.
    fn is_nominal(&self) -> bool {
        matches!(
            self.kind,
            Declared::NewType { .. } | Declared::Union { .. } | Declared::Family { .. }
        )
    }

    /// The type of a signature or alias member, its names read through `elaborator`: alone, or
    /// relative inside a component.
    fn elaborate<R: Reads<'graph> + ?Sized>(
        &self,
        elaborator: &Elaborator<'_, '_, '_, '_, '_, R>,
    ) -> Result<Parametric, Elaboration> {
        match self.kind {
            // An alias is rigid as written.
            Declared::Alias(rhs) => elaborator.part(rhs, &TOP),
            // A signature over a run-bound name is the derived name of the one a binding declares.
            Declared::Signature { group, body } => {
                let mut heads = BumpVec::new_in(elaborator.scratch);
                signature_type(elaborator, group, body, self.site, &mut heads)
            }
            _ => unreachable!("a nominal member seals in a window"),
        }
    }
}

/// A bodyless keyworded head's shape under one of its keys, by the head's site: a shape over the
/// signature's head parameters, or a quantified head's scheme.
pub(super) type HeadShape = (Site, Which, DeclaredType<Parametric>);

/// The shape of each bodyless keyworded head of the signature the `SIG` `node` declares, by the
/// head's site and which of its keys the shape is under — the shapes the signature holds, over its
/// head parameters — its names read through `reader`, which reads where the `SIG` is written.
pub(super) fn signature_heads<'graph, 'x, R: Reads<'graph> + ?Sized>(
    node: &'graph KExpression<'graph>,
    reader: &R,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<(Parametric, BumpVec<'x, HeadShape>), Elaboration> {
    let declaration = Declaration::of(reader.shape(), node, scratch)?;
    let Declared::Signature { group, body } = declaration.kind else {
        return Err(Elaboration::Unsupported {
            site: declaration.site,
        });
    };
    let elaborator = Elaborator {
        reader,
        types,
        scratch,
        fellows: Fellows::NONE,
        locals: &[],
        binder: Cell::new(false),
    };
    let mut heads = BumpVec::new_in(scratch);
    let signature = signature_type(&elaborator, group, body, declaration.site, &mut heads)?;
    Ok((signature, heads))
}

/// A `UNION`'s `#{<Tag>: <payload>, …}` dict as tag/payload pairs, in written order: each key quotes
/// a tag and each value the payload's type code. A repeated tag, or none, is refused.
fn variants<'graph, 'x>(
    part: &'graph ExpressionPart<'graph>,
    scratch: BumpAllocator<'x>,
) -> Option<&'x [(TypeSymbol, &'graph ExpressionPart<'graph>)]> {
    let ExpressionPart::DictLiteral(written) = part else {
        return None;
    };
    if written.is_empty() {
        return None;
    }
    let mut pairs = BumpVec::with_capacity_in(written.len(), scratch);
    for (tag, payload) in written.iter() {
        let ExpressionPart::Type(tag) = *quoted_part(tag)? else {
            return None;
        };
        if pairs.iter().any(|(seen, _)| *seen == tag) {
            return None;
        }
        pairs.push((tag, quoted_part(payload)?));
    }
    Some(pairs.leak())
}

/// The parameter names of a `(<P>… -> <Name>)` declarator, symbol-sorted — the order a family's
/// quantifiers index. A repeated name, or none, is refused.
fn parameters<'x>(
    part: &ExpressionPart<'_>,
    scratch: BumpAllocator<'x>,
) -> Option<&'x [TypeSymbol]> {
    let mut names = BumpVec::new_in(scratch);
    for name in declarator_parameters(part) {
        if names.contains(&name) {
            return None;
        }
        names.push(name);
    }
    names.sort_unstable();
    (!names.is_empty()).then(|| &*names.leak())
}

/// A `SIG`'s signature over its head group and its body's declarators, in source order.
///
/// Each name of the head group is a [`Parameter`](crate::type_lattice::TypeNode::Parameter) under
/// its written bound — a closed one, so a bound naming another parameter is refused. The parameters
/// and the manifest members are the signature's own names, resolved through the elaborator's local
/// table rather than through a mention, since the shape declared them in the definition. A bodyless `EXPR`,
/// `OP` or `UNARY OP` head is a keyworded member, pushed onto `heads` by its site under each of
/// its keys; a bodyless `GROUP` is the operator channel's.
fn signature_type<'graph, 'h, R: Reads<'graph> + ?Sized>(
    elaborator: &Elaborator<'_, '_, '_, '_, '_, R>,
    group: Option<&'graph ExpressionPart<'graph>>,
    body: &'graph ExpressionPart<'graph>,
    site: Site,
    heads: &mut BumpVec<'h, HeadShape>,
) -> Result<Parametric, Elaboration> {
    let unsupported = Elaboration::Unsupported { site };
    let ExpressionPart::ListLiteral(body) = body else {
        return Err(unsupported);
    };
    let scratch = elaborator.scratch;
    let types = elaborator.types;
    let rigid = elaborator.reader.rigid_reads();
    let mut draft = Signing::new(scratch);
    let mut locals: BumpVec<'_, (TypeSymbol, Parametric)> = BumpVec::new_in(scratch);
    if let Some(group) = group {
        let group = elaborator.group(group, &TOP, false)?;
        for (index, (name, bound)) in group.names.iter().zip(&group.bounds).enumerate() {
            if group.names[..index].contains(name) {
                return Err(unsupported);
            }
            let handle = types.head_parameter(*name, *bound);
            draft.insert_parameter(*name, handle);
            locals.push((*name, handle));
        }
    }
    // The groups this signature declares, and the symbols of the heads that state a result of
    // their own — admitted only where the symbol chains pairwise, which a later group may settle.
    let mut groups: BumpVec<'_, DeclaredGroup<'_>> = BumpVec::new_in(scratch);
    let mut returning: BumpVec<'_, (KeywordSymbol, Site)> = BumpVec::new_in(scratch);

    for member in body.iter() {
        let node = (member.as_part().and_then(quoted_body))
            .ok_or(unsupported)?
            .statement_spine();
        let member = Elaborator {
            reader: elaborator.reader,
            types,
            scratch,
            fellows: elaborator.fellows,
            locals: &locals,
            binder: Cell::new(false),
        };
        let site = elaborator.reader.shape().site(&node.parts[0].value);
        let unsupported = Elaboration::Unsupported { site };
        let form = node.cache().builtin_shape().ok_or(unsupported)?;
        let mut name_part = None;
        let mut rhs = None;
        let mut data = None;
        let mut definition = None;
        let mut type_parts = [None; 2];
        let mut type_count = 0;
        for (role, part) in form.roles().zip(node.parts) {
            match role {
                Role::Name => name_part = Some(&part.value),
                Role::Rhs => rhs = Some(&part.value),
                Role::Data => data = Some(&part.value),
                Role::Definition(DefinitionKind::Members) => definition = Some(&part.value),
                Role::TypeExpression => {
                    type_parts[type_count] = Some(&part.value);
                    type_count += 1;
                }
                _ => {}
            }
        }
        match form.id {
            BuiltinShapeId::LetValue => {
                let ExpressionPart::Type(name) = name_part.ok_or(unsupported)? else {
                    return Err(unsupported);
                };
                let handle = member.part(rhs.ok_or(unsupported)?, &TOP)?;
                draft.insert_manifest(*name, handle);
                locals.push((*name, handle));
            }
            BuiltinShapeId::Val => {
                let ExpressionPart::Identifier(name) = name_part.ok_or(unsupported)? else {
                    return Err(unsupported);
                };
                member.binder.set(true);
                let handle = member.part_declared(type_parts[0].ok_or(unsupported)?, &TOP)?;
                draft.insert_value_slot(*name, handle);
            }
            BuiltinShapeId::ExpressionHead | BuiltinShapeId::QuantifiedExpressionHead => {
                member.binder.set(true);
                let shape = member.node_declared(site, node, &TOP)?;
                heads.push((site, Which::Only, shape));
                draft.push_keyworded(shape);
            }
            BuiltinShapeId::OperatorHead
            | BuiltinShapeId::OperatorHeadReturning
            | BuiltinShapeId::UnaryOperatorHeadReturning => {
                let data = data.ok_or(unsupported)?;
                let (shape, bridge) = operator_shape(
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
                // A head's shapes, in the order its bucket keys are: a `UNARY OP`'s unary key, then
                // its binary key.
                let keyed = [Some(shape), bridge];
                let count = keyed.iter().flatten().count();
                heads.extend(
                    keyed
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(index, shape)| (site, Which::of(count, index), shape)),
                );
                draft.push_keyworded(shape);
            }
            // A bodyless `GROUP` declares a chaining record, which the operator channel owns.
            BuiltinShapeId::GroupHeadFoldLeft
            | BuiltinShapeId::GroupHeadFoldRight
            | BuiltinShapeId::GroupHeadPairwiseFoldLeft
            | BuiltinShapeId::GroupHeadPairwiseFoldRight => {
                let mode = group_mode(form.id, data).ok_or(unsupported)?;
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
    let (signed, operands) = draft.finished();
    if elaborator.reader.rigid_reads() == rigid {
        return Ok(sealed_signature(types, scratch, &signed, operands).into());
    }
    // Over a run-bound name, the derived name of the signature a binding declares.
    let term = DerivedTerm::Signature {
        signed: &signed,
        operands,
    };
    let derive = &mut |term: DerivedTerm<'_>| elaborator.reader.marks().derive(term);
    let handle = handed(types, scratch, term, derive).map_err(|unwalked| Elaboration::Unknown {
        site,
        unwalked: unwalked.into(),
    })?;
    Ok(handle.expect("a signature seals at every operand"))
}

/// How a run of a `SIG` body's bodyless `GROUP` head reduces, read off its form id and — for a
/// pairwise head — the combiner it quotes.
fn group_mode(
    form: BuiltinShapeId,
    combiner: Option<&ExpressionPart<'_>>,
) -> Option<ReductionMode> {
    match form {
        BuiltinShapeId::GroupHeadFoldLeft => Some(ReductionMode::FoldLeft),
        BuiltinShapeId::GroupHeadFoldRight => Some(ReductionMode::FoldRight),
        BuiltinShapeId::GroupHeadPairwiseFoldLeft | BuiltinShapeId::GroupHeadPairwiseFoldRight => {
            Some(ReductionMode::Pairwise {
                combiner: quoted_operator(combiner?)?,
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
fn group_members<'graph, 'x, R: Reads<'graph> + ?Sized>(
    elaborator: &Elaborator<'_, '_, '_, '_, '_, R>,
    definition: &'graph ExpressionPart<'graph>,
    mode: ReductionMode,
    draft: &mut Signing<'x>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, KeywordSymbol>, Elaboration> {
    let unsupported = Elaboration::Unsupported {
        site: elaborator.reader.shape().site(definition),
    };
    let ExpressionPart::ListLiteral(heads) = definition else {
        return Err(unsupported);
    };
    let mut members: BumpVec<'x, KeywordSymbol> = BumpVec::new_in(scratch);
    for head in heads.iter() {
        let head = (head.as_part().and_then(quoted_body))
            .ok_or(unsupported)?
            .statement_spine();
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
        let (shape, _) = operator_shape(
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
    symbol_from_quote_body(quoted)
}
