//! Type expressions elaborated into lattice handles: where the program loads and, for what the load
//! leaves unknown, in the walk a settlement or a frame's activation runs under the binding that
//! closes it. The run elaborates none.
//!
//! A type expression is syntax in program storage; its type names are mentions the shape builder
//! already resolved to coordinates. [`type_expression`](expression::type_expression) turns one into a [`KType`] by reading each
//! name through a [`Reads`] and building lattice nodes over what it reads. [`callable_type`](signature::callable_type) reads
//! a callable's function type off the builtin shape node its body sits in and — born for a
//! registration — what the registration's bucket holds: the expression shape ranked by the
//! registration's classes, and how a keyworded call binds its slots. [`type_declarations`](declaration::type_declarations) takes a
//! whole component of type binders and hands back one handle per member, sealing a group of
//! mutually recursive declarations in one window. [`builtin_shape_types`] interns a builtin
//! bucket's own overloads — the `static` slot types of a
//! [`BUILTIN_SHAPES`](crate::parse::BUILTIN_SHAPES) entry — as one handle apiece,
//! and [`builtin_result`] and [`builtin_error`] seal the builtin nominals. [`self_signature`] reads a
//! module's own signature off the activation its body ran in.
//!
//! The load's walk runs each of those doors unit by unit through the type half ([`unit`]): a
//! reader that reads no activation, over a [`TypeContext`] the walk implements, which fixes each
//! fact closed, rigid over the names a run binds, or unknown. [`self_signature`] alone reads an
//! activation, where a type binding holds a [`TypeValue`](crate::values::TypeValue).
//!
//! Elaborated: a bare type name, `LIST OF Elem`, `MAP Key -> Val`, `FN :{…} -> Ret`,
//! `EXPR #(head) -> Ret` with and without `FOR ALL` and ranked where a signature member writes an
//! integer in a slot's place, a union `A | B` and a meet `A & B` of members, a record type `:{…}`, a union member `Union.Tag`,
//! the declared type of a record's field `Record.field`, and a constructor application `Pair {Key =
//! Number}` with its arity-one sugar `Number AS Wrap`. A name a `FOR ALL` group declares is that
//! group's quantifier, bounded by what `(Name UNDER <bound>)` writes or else by `Any`, and is never
//! a mention; a `SIG`'s head group (`SIG <Name> FOR ALL <group> = …`) bounds each head parameter
//! the same way. Every other spelling is [`Unsupported`].
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
mod declaration;
mod expression;
mod members;
mod module;
mod paths;
mod reads;
mod signature;
mod unit;

#[cfg(test)]
mod tests;

pub use builtin::{builtin_error, builtin_result, builtin_shape_types};
pub use declaration::declares_nominal;
pub use expression::declared_field;
pub use members::{
    SignatureMember, applied_member, member_count, schema_member, schema_of, signature_member,
    type_members, value_count,
};
pub use module::{module_signature, self_signature};
pub use paths::{declared_through, path_binding};
pub use reads::{Reads, TypeAt};
pub use unit::{
    Mode, Reader, Signatures, Tools, TypeContext, callable, declarations, expression, guards,
    own_levels, registration,
};

#[cfg(test)]
thread_local! {
    /// How many walks this thread is inside: a type expression elaborated outside every walk is
    /// the run's, which elaborates none.
    static WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Type expressions elaborated inside a walk, then outside every walk. Tests reset it.
    pub(crate) static ELABORATED: std::cell::Cell<(usize, usize)> =
        const { std::cell::Cell::new((0, 0)) };
}

/// A walk this thread is inside, for as long as it is held.
#[cfg(test)]
pub(crate) struct Walking;

#[cfg(test)]
impl Walking {
    pub(crate) fn enter() -> Self {
        WALKS.with(|walks| walks.set(walks.get() + 1));
        Walking
    }
}

#[cfg(test)]
impl Drop for Walking {
    fn drop(&mut self) {
        WALKS.with(|walks| walks.set(walks.get() - 1));
    }
}

/// Count one type expression elaborated, inside a walk or outside every walk.
#[cfg(test)]
fn elaborated() {
    let walking = WALKS.with(std::cell::Cell::get) > 0;
    ELABORATED.with(|counts| {
        let (inside, outside) = counts.get();
        counts.set(match walking {
            true => (inside + 1, outside),
            false => (inside, outside + 1),
        });
    });
}
