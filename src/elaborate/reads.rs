//! What an elaboration reads its type names through: the activation a type expression is read in,
//! or — where the program loads, before any activation exists — the load pass's own reader
//! ([`channel`](super::channel)).
//!
//! Every name a type expression reads is a mention the shape builder resolved to a
//! [`Coordinate`]; a reader answers the shape holding those mentions and what a coordinate's
//! binding holds, as a [`TypeAt`]. An activation answers a type or not one; the load-time reader
//! may also answer a rigid variable standing for a type a run binds, or that it cannot know.

use crate::memory::{Bump, BumpAllocator};
use crate::parse::ExpressionPart;
use crate::scope::{Activation, ActivationView, BodyShape, Coordinate, Elaboration, Site};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, TypeNode, TypeRegistry, substitute_levels,
};
use crate::values::{KnottedFamily, Value};

use super::expression::type_expression;
use super::members::{SignatureMember, signature_member};

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

/// The type member `name` of a module of signature `signature`, reached through the value members
/// `chain` names, each step's type read off the one before it ([`signature_member`]). `None` where
/// `signature` is no signature type; [`TypeAt::NotAType`] where a step names no value member at a
/// type, the last names no type member, or a member names a head parameter its application leaves
/// unpinned.
fn member_through(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    signature: KType,
    chain: &[ValueSymbol],
    name: TypeSymbol,
) -> Option<TypeAt> {
    if !matches!(
        types.node(signature),
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. }
    ) {
        return None;
    }
    // A member read at a type, every head parameter it names pinned.
    let read =
        |of: KType, name: BinderSymbol| match signature_member(types, scratch, of.into(), name) {
            Some(SignatureMember {
                declared: DeclaredType::Type(declared),
                unpinned: None,
            }) => types.concrete(declared),
            _ => None,
        };
    let mut held = signature;
    for step in chain {
        match read(held, BinderSymbol::Value(*step)) {
            Some(inner) => held = inner,
            None => return Some(TypeAt::NotAType),
        }
    }
    Some(read(held, BinderSymbol::Type(name)).map_or(TypeAt::NotAType, TypeAt::Type))
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
        let scratch = Bump::new();
        Some((
            member_through(types, &scratch, signature, chain, name)?,
            signature,
        ))
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
