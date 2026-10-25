//! What an elaboration reads its type names through: the load pass's reader
//! ([`unit`](super::unit)). The run elaborates nothing: every type it reads is a fact a walk fixed.
//!
//! Every name a type expression reads is a mention the shape builder resolved to a
//! [`Coordinate`]; a reader answers the shape holding those mentions and what a coordinate's
//! binding holds, as a [`TypeAt`]: a type, a rigid variable standing for a type a run binds, or
//! that it cannot know. A path through a module, `m.Carrier`, reads as its path variable.

use crate::scope::{BodyShape, Coordinate, Site};
use crate::symbols::{TypeSymbol, ValueSymbol};
use crate::type_lattice::{KType, Parametric};
use crate::values::Chain;

/// What a reader answers for the binding at a coordinate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeAt {
    /// It holds this type.
    Type(KType),
    /// A run binds it: this lexical variable stands for its type.
    Rigid(Parametric),
    /// It holds something other than a type.
    NotAType,
    /// This reader cannot know it before the program runs.
    Unknown,
}

/// A reader of the names a type expression mentions.
pub trait Reads<'graph> {
    /// The shape whose mentions the expression's names are.
    fn shape(&self) -> &'graph BodyShape<'graph>;

    /// What the binding at `at` holds.
    fn type_at(&self, at: Coordinate) -> TypeAt;

    /// How many [`TypeAt::Rigid`] answers this reader has given: what a spelling that must not be
    /// elaborated over a rigid variable compares before and after reading its operands.
    fn rigid_reads(&self) -> u32 {
        0
    }

    /// The type member `name` of the module the binding at `at` holds, reached through the value
    /// members `chain` names in turn — the path spelled at `site` — beside that module's
    /// signature: `None` where the binding holds no module, and [`TypeAt::NotAType`] where the
    /// path reads no type. The load answers the path's variable.
    fn member_type(
        &self,
        _at: Coordinate,
        _site: Site,
        _chain: &[ValueSymbol],
        _name: TypeSymbol,
        _types: &Chain<'_, '_, '_, '_>,
    ) -> Option<(TypeAt, KType)> {
        Some((TypeAt::Unknown, KType::EMPTY_SIGNATURE))
    }

    /// The type member `name` of the module the expression at `root`, written in place, builds,
    /// reached through the value members `chain` names — the path spelled at `site`. The load
    /// answers the path's variable.
    fn inline_member(
        &self,
        _site: Site,
        _root: Site,
        _chain: &[ValueSymbol],
        _name: TypeSymbol,
    ) -> TypeAt {
        TypeAt::Unknown
    }
}
