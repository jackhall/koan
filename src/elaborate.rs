//! Type expressions elaborated into lattice handles, where the program loads and, for what the load
//! leaves unknown, where they are read.
//!
//! A type expression is syntax in program storage; its type names are mentions the shape builder
//! already resolved to coordinates. [`type_expression`] turns one into a [`KType`] by reading each
//! name through a [`Reads`] and building lattice nodes over what it reads. [`callable_type`] reads
//! a callable's function type off the builtin shape node its body sits in and — born for a
//! registration — what the registration's bucket holds: the expression shape ranked by the
//! registration's classes, and how a keyworded call binds its slots. [`type_declarations`] takes a
//! whole component of type binders and hands back one handle per member, sealing a group of
//! mutually recursive declarations in one window. [`builtin_shape_types`] interns a builtin
//! bucket's own overloads — the `static` slot types of a
//! [`BUILTIN_SHAPES`](crate::parse::builtin_shapes::BUILTIN_SHAPES) entry — as one handle apiece,
//! and [`builtin_result`] and [`builtin_error`] seal the builtin nominals. [`self_signature`] reads a
//! module's own signature off the activation its body ran in.
//!
//! [`type_channel`] is the load pass: it runs each of those doors over a loaded program's shapes
//! through a reader that reads no activation, and writes what it fixes into the shapes' write-once
//! cells — closed, rigid over the names a run binds, or unknown. Everything else reads through an
//! activation, where a type binding holds a [`TypeValue`](crate::values::TypeValue).
//!
//! Elaborated: a bare type name, `LIST OF Elem`, `MAP Key -> Val`, `FN :{…} -> Ret`,
//! `EXPR #(head) -> Ret` with and without `FOR ALL` and ranked where a signature member writes an
//! integer in a slot's place, a union `A | B` and a meet `A & B` of members — refused where two
//! signatures rank one keyword pattern two ways — a record type `:{…}`, a union member `Union.Tag`,
//! the declared type of a record's field `Record.field`, and a constructor application `Pair {Key =
//! Number}` with its arity-one sugar `Number AS Wrap`. A name a `FOR ALL` group declares is that
//! group's quantifier, bounded by what `(Name UNDER <bound>)` writes or else by `Any`, and is never
//! a mention; a `SIG`'s `TYPE (Name UNDER <bound>)` bounds its abstract member the same way. Every
//! other spelling is [`Unsupported`].
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::memory`,
//! `crate::parse`, `crate::scope`, `crate::source`, `crate::symbols`, `crate::type_lattice` and
//! `crate::values`, and nothing else in the crate; `tests::boundary` reads the source to hold it
//! there.
//!
//! See [elaborate/README.md](elaborate/README.md).
//!
//! [`Unsupported`]: crate::scope::Elaboration::Unsupported

mod builtin;
mod channel;
mod declaration;
mod expression;
mod module;
mod reads;
mod signature;

#[cfg(test)]
mod tests;

pub use builtin::{builtin_error, builtin_result, builtin_shape_types};
pub use channel::type_channel;
pub use declaration::type_declarations;
pub use expression::{declared_field, type_expression};
pub use module::self_signature;
pub use reads::{Reads, TypeAt};
pub use signature::callable_type;
