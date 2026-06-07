//! `KKind` — the shallow dispatch *kind* of a type. A type-accepting argument slot carries
//! a kind expectation as [`KType::OfKind`](super::ktype::KType::OfKind); a type value flowing
//! into such a slot is classified by [`KType::kind_of`](super::ktype::KType::kind_of) and
//! matched against it.
//!
//! See [type-language via dispatch](../../../../design/typing/type-language-via-dispatch.md).

/// Shallow kind of a type, used to admit a type value into a type-accepting slot.
///
/// `kind_of` (the value-classification direction) yields only [`KKind::Proper`],
/// [`KKind::Module`], or [`KKind::Signature`] — a runtime type is always one of those.
/// [`KKind::Any`] is a *slot* expectation only ("accepts any type value"), never the
/// classification of a value.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KKind {
    /// A proper (non-module, non-signature) type value — `SetRef`, `List`, `Record`,
    /// `KFunction`, a bare resolved leaf, etc. As a slot it is the lowest-specificity type
    /// slot (ties with `Identifier` in `is_more_specific_than`).
    Proper,
    /// A first-class module value's kind, and the `:Module` wildcard slot.
    ///
    /// The module-satisfies-a-signature *constraint* role lives on the separate
    /// [`KType::Signature`](super::ktype::KType::Signature) slot, so the dispatch *kind* of a
    /// module needs no sig payload here. A future unification could fold that constraint in as
    /// `Module(Some(sig))`.
    Module,
    /// A first-class signature value's kind, and the `:Signature` wildcard slot. The
    /// sig-as-constraint role is the separate [`KType::Signature`](super::ktype::KType::Signature).
    Signature,
    /// A slot accepting any (proper) type value (the `:Type` surface). Like `Proper` it does
    /// not admit module / signature values — bare `Type` denotes "any type value", and the
    /// design pins the module/sig seam to the narrower `:Module` / `:Signature` slots. More
    /// specific than `Proper` for tie-breaking.
    Any,
    // Constructor(arity) — the `* -> *` arity tower for higher-kinded type constructors —
    // is the deferred extension point for higher-kinded type-constructor work. This enum
    // ships the shallow kinds only.
}
