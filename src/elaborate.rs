//! Type expressions elaborated into lattice handles, read where they are written.
//!
//! A type expression is syntax in program storage; its type names are mentions the shape builder
//! already resolved to coordinates. [`type_expression`] turns one into a [`KType`] by reading each
//! name through the activation the expression is read in — a type binding holds a
//! [`TypeValue`](crate::values::TypeValue) — and building lattice nodes over what it reads.
//! [`callable_type`] reads a callable's function type off the builtin shape node its body sits in,
//! where the callable is born, and — born for a registration — what the registration's bucket
//! holds: the expression shape ranked by the registration's classes, and how a keyworded call
//! binds its slots. [`static_callable_type`] reads the same where the program loads, through the
//! builtin table alone. [`builtin_shape_types`] interns a builtin bucket's own overloads — the
//! `static` slot types of a [`BUILTIN_SHAPES`](crate::parse::builtin_shapes::BUILTIN_SHAPES) entry
//! — as one handle apiece, and [`builtin_result`] and [`builtin_error`] seal the builtin nominals.
//! [`type_declarations`] takes a whole component of type binders and hands back one handle per
//! member, sealing a group of mutually recursive declarations in one window. [`self_signature`]
//! reads a module's own signature off the activation its body ran in.
//!
//! A door that elaborates a type expression reads its names through a [`Reads`]: an activation, or
//! [`BuiltinsOnly`].
//!
//! Elaborated: a bare type name, `LIST OF Elem`, `MAP Key -> Val`, `FN :{…} -> Ret`,
//! `EXPR #(head) -> Ret` with and without `FOR ALL` and ranked where a signature member writes an
//! integer in a slot's place, a union `A | B` and a meet `A & B` of members — refused where two
//! signatures rank one keyword pattern two ways — a record type `:{…}`, a union member `Union.Tag`,
//! the declared type of a record's field `Record.field`, and a constructor application `Pair {Key = Number}` with its arity-one sugar
//! `Number AS Wrap`. A name a `FOR ALL` group declares is that group's quantifier, bounded by what
//! `(Name UNDER <bound>)` writes or else by `Any`, and is never a mention; a `SIG`'s
//! `TYPE (Name UNDER <bound>)` bounds its abstract member the same way. Every other spelling is
//! [`Unsupported`].
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
mod reads;
mod signature;

#[cfg(test)]
mod tests;

pub use builtin::{builtin_error, builtin_result, builtin_shape_types};
pub use declaration::type_declarations;
pub use expression::{declared_field, type_expression};
pub use module::self_signature;
pub use reads::{BuiltinsOnly, Reads};
pub use signature::{
    Callable, Canonical, ParameterBinding, Registered, callable_type, static_callable_type,
};

use std::fmt;

use crate::scope::Site;
use crate::symbols::{Symbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{KType, TypeRegistry, display_name};

/// Why a type expression did not elaborate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elaboration {
    /// The type name at `site` is bound to something other than a type.
    NotAType { name: TypeSymbol, site: Site },
    /// A spelling this module does not elaborate, at `site`.
    Unsupported { site: Site },
    /// A projection `Owner.name` naming a member `Owner` does not declare: a union's tag, or a
    /// record's field.
    NoSuchMember { owner: KType, name: Symbol },
    /// A bound at `site` that names a type variable — a `FOR ALL` name or a signature's abstract
    /// member — or is `Never`.
    Bound { site: Site },
    /// A meet at `site` of two signatures that rank one keyword pattern two ways.
    RankingDisagrees { site: Site },
}

impl Elaboration {
    /// The refusal as an error value's message, with its names spelled through `symbols` and its
    /// types through `types`.
    pub fn display<'x, 'run>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x TypeRegistry<'run>,
    ) -> ElaborationDisplay<'x, 'run> {
        ElaborationDisplay {
            error: self,
            symbols,
            types,
        }
    }
}

/// An [`Elaboration`] beside the interner and registry it renders through.
pub struct ElaborationDisplay<'x, 'run> {
    error: &'x Elaboration,
    symbols: &'x SymbolInterner,
    types: &'x TypeRegistry<'run>,
}

impl fmt::Display for ElaborationDisplay<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.error {
            Elaboration::NotAType { name, .. } => {
                write!(f, "{} names no type", self.symbols.display(name.symbol()))
            }
            Elaboration::Unsupported { .. } => f.write_str("this type expression is not supported"),
            Elaboration::NoSuchMember { owner, name } => write!(
                f,
                "{} has no member {}",
                display_name(*owner, self.types, self.symbols),
                self.symbols.display(*name)
            ),
            Elaboration::Bound { .. } => f.write_str("a bound names a type variable or Never"),
            Elaboration::RankingDisagrees { .. } => {
                f.write_str("a meet of two signatures ranks one key two ways")
            }
        }
    }
}
