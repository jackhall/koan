//! **Nominals over run-bound names**: a component of type binders holding a `NEWTYPE`, a `UNION`
//! or a family whose declared parts read a name a run binds — `NEWTYPE Box = :{v :Elt}` under
//! `FOR ALL #[Elt]`. No lattice node spells a fresh identity over a variable, so each member is a
//! derived name ([`DerivedTerm::Nominal`](super::DerivedTerm::Nominal)) over the group the
//! declaration door read ([`Nominals`]), its operands each representation and each relative
//! fellow written over names.
//!
//! The door seals a closed group, and the term seals the group a binding closes, through one
//! closed rule, [`sealed_nominals`], so a binding written in declares the identity a fresh load
//! with that binding declared gives: one binding's `Box` is one type, and a `Box` at `Number` and
//! one at `Str` are two.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::symbols::TypeSymbol;
use crate::type_lattice::{KKind, KType, Parametric, RecursiveGroupWindow, RelativeSchema};
use crate::values::Chain;

/// A nominal component as the declaration door read it, every type it read an operand of the term
/// over it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Nominals<'a> {
    /// The window's members in announcement order: each name, the binder owning it where it is a
    /// union's variant, and its kind.
    pub members: &'a [(TypeSymbol, Option<TypeSymbol>, KKind)],
    /// Each union binder beside the window members it owns.
    pub binders: &'a [(TypeSymbol, &'a [usize])],
    /// Each window member's schema, parallel to `members`.
    pub schemas: &'a [Schema<'a>],
    /// How each component member's handle is read off the sealed group, in member order.
    pub handles: &'a [Handed],
}

/// A window member's schema: a newtype's representation, or a family's beside its parameter
/// names, each representation an operand by its index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Schema<'a> {
    NewType(u16),
    Constructor {
        representation: Option<u16>,
        params: &'a [TypeSymbol],
    },
}

/// How a component member's handle is read off its sealed group: a window member's, a union
/// binder's, or a signature or alias fellow's absolute twin of its relative type, an operand by its
/// index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Handed {
    Member(usize),
    Binder(TypeSymbol),
    Relative(u16),
}

impl<'a> Nominals<'a> {
    /// The group laid through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Nominals<'to> {
        let binders =
            (self.binders.iter()).map(|(name, owned)| (*name, collect(lay, owned.iter().copied())));
        let binders = collect(lay, binders);
        let schemas = self.schemas.iter().map(|schema| match *schema {
            Schema::NewType(at) => Schema::NewType(at),
            Schema::Constructor {
                representation,
                params,
            } => Schema::Constructor {
                representation,
                params: collect(lay, params.iter().copied()),
            },
        });
        Nominals {
            members: collect(lay, self.members.iter().copied()),
            binders,
            schemas: collect(lay, schemas),
            handles: collect(lay, self.handles.iter().copied()),
        }
    }
}

/// The handles of `nominals`'s component members, in member order: its window filled with each
/// schema at the closed `operands` — a newtype's representation, a family's over its parameters'
/// quantifiers — sealed, and each relative fellow made absolute. `None` where the window seals no
/// member a handle reads.
pub fn sealed_nominals<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    nominals: &Nominals<'_>,
    operands: &[Parametric],
) -> Option<&'x [KType]> {
    let window = RecursiveGroupWindow::for_component(scratch, nominals.members, nominals.binders);
    let operand = |at: u16| operands.get(usize::from(at)).copied();
    let mut sealed = None;
    for (index, schema) in nominals.schemas.iter().enumerate() {
        let schema = match *schema {
            Schema::NewType(at) => RelativeSchema::NewType(types.concrete(operand(at)?)?),
            Schema::Constructor {
                representation,
                params,
            } => {
                let representation = match representation {
                    Some(at) => Some(operand(at)?),
                    None => None,
                };
                RelativeSchema::constructor(scratch, scratch, representation, params)
            }
        };
        sealed = window.fill_member(index, schema, types, scratch);
    }
    let sealed = sealed?;
    let mut handles = BumpVec::with_capacity_in(nominals.handles.len(), scratch);
    for handed in nominals.handles {
        handles.push(match *handed {
            Handed::Member(index) => sealed.member(index)?,
            Handed::Binder(name) => sealed.binder_type(name)?,
            Handed::Relative(at) => sealed.absolute(types, scratch, types.concrete(operand(at)?)?),
        });
    }
    Some(handles.leak())
}
