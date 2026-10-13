//! What an elaboration reads its type names through: the activation a type expression is read in,
//! or — where the program loads, before any activation exists — the load pass's own reader
//! ([`channel`](super::channel)).
//!
//! Every name a type expression reads is a mention the shape builder resolved to a
//! [`Coordinate`]; a reader answers the shape holding those mentions and what a coordinate's
//! binding holds, as a [`TypeAt`]. An activation answers a type or not one; the load-time reader
//! may also answer a rigid variable standing for a type a run binds, or that it cannot know. A path
//! through a module, `m.Carrier`, is read where it runs as the run binds its path variable
//! ([`bound_path`]), and at load as that variable.

use crate::memory::{Bump, BumpAllocator};
use crate::parse::ExpressionPart;
use crate::scope::{Activation, ActivationView, BodyShape, Coordinate, Elaboration, Site};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{KType, Parametric, TypeNode, substitute_levels};
use crate::values::{KnottedFamily, Types, Value};

use super::expression::type_expression;
use super::paths::{bound_path, member_module, member_type};

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
        _types: &Types<'_, '_>,
    ) -> Option<(TypeAt, KType)> {
        Some((TypeAt::Unknown, KType::EMPTY_SIGNATURE))
    }

    /// The type member `name` of the module the expression at `root`, written in place, builds,
    /// reached through the value members `chain` names — the path spelled at `site`. The load
    /// answers the path's variable; where it runs, the run never builds that module, and the type
    /// is unknown.
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

/// The type member `name` of a module of signature `signature`, reached through the value members
/// `chain` names, each step's type read off the one before it ([`member_module`]). `None` where
/// `signature` is no signature type; [`TypeAt::NotAType`] where a step names no value member at a
/// type, the last names no type member, or a member names a head parameter its application leaves
/// unpinned.
fn member_through(
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'_>,
    signature: KType,
    chain: &[ValueSymbol],
    name: TypeSymbol,
) -> Option<TypeAt> {
    if !matches!(
        types.node(signature),
        TypeNode::Signature { .. }
            | TypeNode::SignatureApply { .. }
            | TypeNode::SignatureMeet { .. }
    ) {
        return None;
    }
    let read = member_module(types, scratch, signature, chain)
        .and_then(|inner| member_type(types, scratch, inner, BinderSymbol::Type(name)));
    Some(read.map_or(TypeAt::NotAType, TypeAt::Type))
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
        site: Site,
        chain: &[ValueSymbol],
        name: TypeSymbol,
        types: &Types<'_, '_>,
    ) -> Option<(TypeAt, KType)> {
        let held = self.read(at);
        held.as_module()?;
        let signature = held.concrete_ktype();
        // The load records every path its type expressions spell; one it read through a declared
        // application binds as the run's path variable does.
        if let Some(read) = self.shape().path_read(site)
            && read.entry.through.is_some()
        {
            let bound = bound_path(self, types, at, read.entry);
            return Some((bound.map_or(TypeAt::NotAType, TypeAt::Type), signature));
        }
        let scratch = Bump::new();
        Some((
            member_through(types, &scratch, signature, chain, name)?,
            signature,
        ))
    }

    /// A path through a module built in place reads as the type the load resolved it to: the run
    /// builds no module there.
    fn inline_member(
        &self,
        site: Site,
        _root: Site,
        _chain: &[ValueSymbol],
        _name: TypeSymbol,
    ) -> TypeAt {
        let read = self.shape().path_read(site);
        match read.and_then(|read| read.entry.closed()) {
            Some(closed) => TypeAt::Type(closed),
            None => TypeAt::Unknown,
        }
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
        site: Site,
        chain: &[ValueSymbol],
        name: TypeSymbol,
        types: &Types<'_, '_>,
    ) -> Option<(TypeAt, KType)> {
        ActivationView::member_type(self, at, site, chain, name, types)
    }

    fn inline_member(
        &self,
        site: Site,
        root: Site,
        chain: &[ValueSymbol],
        name: TypeSymbol,
    ) -> TypeAt {
        ActivationView::inline_member(self, site, root, chain, name)
    }
}

/// The type the type part `part` of `view`'s shape denotes where it runs: the load fixed it where
/// it could, and only what it left unknown is elaborated here.
pub fn denoted<'graph, XF: KnottedFamily<'graph>>(
    part: &'graph ExpressionPart<'graph>,
    view: &ActivationView<'graph, '_, XF>,
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Elaboration> {
    // Where it runs, every name a type reads is bound to a concrete type.
    let concrete = |kt| {
        types
            .concrete(kt)
            .expect("a type read where it runs holds no variable")
    };
    let elaborated = || type_expression(part, view, types, scratch).map(concrete);
    let loaded = view.shape().typed_expression(Site::of(part)).solved(
        view,
        scratch,
        &|at, entry| bound_path(view, types, at, entry),
        |value, bindings| Some(concrete(substitute_levels(types, scratch, value, bindings))),
    );
    debug_assert!(
        loaded.is_none() || loaded == elaborated().ok(),
        "the load-time type agrees with elaborating where it runs"
    );
    loaded.map_or_else(elaborated, Ok)
}
