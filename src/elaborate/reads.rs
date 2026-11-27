//! What an elaboration reads its type names through: the load pass's reader
//! ([`unit`](super::unit)). The run elaborates nothing: every type it reads is a fact a walk fixed.
//!
//! Every name a type expression reads is a mention the shape builder resolved to a
//! [`Coordinate`]; a reader answers the shape holding those mentions and what a coordinate's
//! binding holds, as a [`TypeAt`]: a type, a rigid variable standing for a type a run binds, or
//! that it cannot know. A path through a module, `m.Carrier`, reads as its path variable.

use crate::scope::{AsWritten, BodyShape, Coordinate, Site, Unresolved, Unwalked};
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
    /// This reader cannot know it before the program runs: no binding is written in here.
    Unknown(Unwalked),
}

/// A reader of the names a type expression mentions.
pub trait Reads<'graph> {
    /// Why its namer names no derived name: never, for a walk's reader.
    type Unwalked: Into<Unwalked>;

    /// The shape whose mentions the expression's names are.
    fn shape(&self) -> &'graph BodyShape<'graph>;

    /// What the binding at `at` holds.
    fn type_at(&self, at: Coordinate) -> TypeAt;

    /// How many [`TypeAt::Rigid`] answers this reader has given: what a spelling that must not be
    /// elaborated over a rigid variable compares before and after reading its operands.
    fn rigid_reads(&self) -> u32 {
        0
    }

    /// The bound the lexical variable `variable` of a run-bound name was written with, where a
    /// message names it so.
    fn written_bound(
        &self,
        variable: Parametric,
        types: &Chain<'_, '_, '_, '_>,
    ) -> Option<AsWritten> {
        let _ = (variable, types);
        None
    }

    /// Where a judgement over what this reader read marks, and what names its derived names.
    fn marks(&self) -> &dyn super::judge::Derives<Unwalked = Self::Unwalked>;

    /// The type member `name` of the module the binding at `at` holds, reached through the value
    /// members `chain` names in turn — the path spelled at `site` — beside that module's
    /// signature: `None` where the binding holds no module, and [`TypeAt::NotAType`] where the
    /// path reads no type. The load answers the path's variable, or asks for the path's resolution
    /// where the walk holds none yet.
    fn member_type(
        &self,
        at: Coordinate,
        site: Site,
        chain: &[ValueSymbol],
        name: TypeSymbol,
        types: &Chain<'_, '_, '_, '_>,
    ) -> Result<Option<(TypeAt, KType)>, Unresolved>;

    /// The type member `name` of the module the expression at `root`, written in place, builds,
    /// reached through the value members `chain` names — the path spelled at `site`. The load
    /// answers the path's variable, or asks for the path's resolution where the walk holds none.
    fn inline_member(
        &self,
        site: Site,
        root: Site,
        chain: &[ValueSymbol],
        name: TypeSymbol,
    ) -> Result<TypeAt, Unresolved>;
}
