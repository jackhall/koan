//! Type expressions elaborated into lattice handles, read where they are written.
//!
//! A type expression is syntax in program storage; its type names are mentions the shape builder
//! already resolved to coordinates. [`type_expression`] turns one into a [`KType`] by reading each
//! name through the activation the expression is read in — a type binding holds a
//! [`TypeValue`](crate::values::TypeValue) — and building lattice nodes over what it reads.
//! [`callable_type`] reads a callable's signature and return off the builtin shape node its body
//! sits in, where the callable is born, and [`builtin_shape_types`] interns a builtin bucket's own
//! overloads — the `static` slot types of a
//! [`BUILTIN_SHAPES`](crate::parse::builtin_shapes::BUILTIN_SHAPES) entry — as one handle apiece.
//! [`type_declarations`] takes a whole component of type binders and hands back one handle per
//! member, sealing a group of mutually recursive declarations in one window. [`self_signature`]
//! reads a module's own signature off the activation its body ran in.
//!
//! Elaborated: a bare type name, `LIST OF Elem`, `MAP Key -> Val`, `FN :{…} -> Ret`,
//! `EXPR (head) -> Ret` with and without `FOR ALL`, a union of members, a record type `:{…}`, a
//! union member `Union.Tag`, and a constructor application `Pair {Key = Number}` with its
//! arity-one sugar `Number AS Wrap`. A name a `FOR ALL` group declares is that group's quantifier,
//! and is never a mention. Every other spelling is [`Unsupported`].
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::memory`,
//! `crate::parse`, `crate::scope`, `crate::type_lattice` and `crate::values`, and nothing else in
//! the crate; `tests::boundary` reads the source to hold it there.
//!
//! See [elaborate/README.md](elaborate/README.md).
//!
//! [`Unsupported`]: Elaboration::Unsupported

mod builtin;
mod declaration;
mod expression;
mod module;
mod signature;

#[cfg(test)]
mod tests;

pub use builtin::builtin_shape_types;
pub use declaration::type_declarations;
pub use expression::type_expression;
pub use module::{Unsigned, self_signature};
pub use signature::callable_type;

use crate::memory::CellHandle;
use crate::parse::{Symbol, TypeSymbol};
use crate::scope::Site;
use crate::type_lattice::KType;

/// Why a type expression did not elaborate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elaboration {
    /// The binder of the type name `name` is still running.
    Pending {
        name: TypeSymbol,
        binder: CellHandle,
    },
    /// The type name at `site` is bound to something other than a type.
    NotAType { name: TypeSymbol, site: Site },
    /// A spelling this module does not elaborate, at `site`.
    Unsupported { site: Site },
    /// A union member projection naming a tag the union does not declare.
    NoSuchMember { union: KType, tag: Symbol },
}
