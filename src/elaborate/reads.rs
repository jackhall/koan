//! What an elaboration reads its type names through: the activation a type expression is read in,
//! or — where the program loads, before any activation exists — the load pass's own reader
//! ([`channel`](super::channel)).
//!
//! Every name a type expression reads is a mention the shape builder resolved to a
//! [`Coordinate`]; a reader answers the shape holding those mentions and what a coordinate's
//! binding holds, as a [`TypeAt`]. An activation answers a type or not one; the load-time reader
//! may also answer a rigid variable standing for a type a run binds, or that it cannot know.

use crate::memory::BumpAllocator;
use crate::parse::ExpressionPart;
use crate::scope::{Activation, ActivationView, BodyShape, Coordinate, Elaboration, Site};
use crate::symbols::{TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, TypeNode, TypeRegistry, member, substitute_levels,
};
use crate::values::{KnottedFamily, Value};

use super::expression::type_expression;

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
    /// members `chain` names in turn, beside that module's signature: `None` where the binding
    /// holds no module, and [`TypeAt::NotAType`] where the module declares no such member. The load
    /// cannot know what a module holds, and answers `Unknown`.
    fn member_type(
        &self,
        _at: Coordinate,
        _chain: &[ValueSymbol],
        _name: TypeSymbol,
        _types: &TypeRegistry<'_>,
    ) -> Option<(TypeAt, KType)> {
        Some((TypeAt::Unknown, KType::EMPTY_SIGNATURE))
    }
}

/// The manifest member `name` of a module whose signature is `signature`, reached through the value
/// members `chain` names — a nested module's value slot is its own signature. `None` where
/// `signature` is no module's; [`TypeAt::NotAType`] where a step names no such member.
fn manifest_through(
    types: &TypeRegistry<'_>,
    signature: KType,
    chain: &[ValueSymbol],
    name: TypeSymbol,
) -> Option<TypeAt> {
    let TypeNode::Signature { schema, .. } = types.node(signature) else {
        return None;
    };
    let Some((first, rest)) = chain.split_first() else {
        let held = member(schema.manifest_members, name).and_then(|held| types.concrete(held));
        return Some(held.map_or(TypeAt::NotAType, TypeAt::Type));
    };
    let inner = member(schema.value_slots, *first)
        .and_then(DeclaredType::as_type)
        .and_then(|inner| types.concrete(inner));
    match inner {
        Some(inner) => manifest_through(types, inner, rest, name).or(Some(TypeAt::NotAType)),
        None => Some(TypeAt::NotAType),
    }
}

impl<'graph, XF: KnottedFamily<'graph>> Reads<'graph> for ActivationView<'graph, '_, XF> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        ActivationView::shape(self)
    }

    fn type_at(&self, at: Coordinate) -> TypeAt {
        match self.read(at) {
            Value::Type(value) => TypeAt::Type(value.handle()),
            _ => TypeAt::NotAType,
        }
    }

    fn member_type(
        &self,
        at: Coordinate,
        chain: &[ValueSymbol],
        name: TypeSymbol,
        types: &TypeRegistry<'_>,
    ) -> Option<(TypeAt, KType)> {
        let held = self.read(at);
        held.as_module()?;
        let signature = held.concrete_ktype();
        Some((manifest_through(types, signature, chain, name)?, signature))
    }
}

impl<'graph, XF: KnottedFamily<'graph>> Reads<'graph> for Activation<'graph, '_, XF> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        ActivationView::shape(self)
    }

    fn type_at(&self, at: Coordinate) -> TypeAt {
        ActivationView::type_at(self, at)
    }

    fn member_type(
        &self,
        at: Coordinate,
        chain: &[ValueSymbol],
        name: TypeSymbol,
        types: &TypeRegistry<'_>,
    ) -> Option<(TypeAt, KType)> {
        ActivationView::member_type(self, at, chain, name, types)
    }
}

/// The type the type part `part` of `view`'s shape denotes where it runs: the load fixed it where
/// it could, and only what it left unknown is elaborated here.
pub fn denoted<'graph, XF: KnottedFamily<'graph>>(
    part: &'graph ExpressionPart<'graph>,
    view: &ActivationView<'graph, '_, XF>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Elaboration> {
    // Where it runs, every name a type reads is bound to a concrete type.
    let concrete = |kt| {
        types
            .concrete(kt)
            .expect("a type read where it runs holds no variable")
    };
    let elaborated = || type_expression(part, view, types, scratch).map(concrete);
    let loaded =
        view.shape()
            .typed_expression(Site::of(part))
            .solved(view, scratch, |value, bindings| {
                Some(concrete(substitute_levels(types, scratch, value, bindings)))
            });
    debug_assert!(
        loaded.is_none() || loaded == elaborated().ok(),
        "the load-time type agrees with elaborating where it runs"
    );
    loaded.map_or_else(elaborated, Ok)
}
