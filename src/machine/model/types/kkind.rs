//! `KKind` — the shallow dispatch *kind* of a type. A type-accepting argument slot carries
//! a kind expectation as [`KType::OfKind`](super::ktype::KType::OfKind); a type value flowing
//! into such a slot is classified by [`KType::kind_of`](super::ktype::KType::kind_of) and
//! matched against it. `OfKind` is **type-channel only** — it admits a type value, never a
//! runtime instance (a value is matched by a type, never by a kind).
//!
//! See [type-language via dispatch](../../../../design/typing/type-language-via-dispatch.md).

/// Shallow kind of a type, used to admit a type value into a type-accepting slot. The kinds
/// form one subsumption lattice:
///
/// ```text
/// AnyType > { Module, Signature, ProperType > { NewType, TypeConstructor } }
/// ```
///
/// [`AnyType`](KKind::AnyType) is a *slot* expectation only ("accepts any proper type value"),
/// never a value classification produced by [`kind_of`](super::ktype::KType::kind_of).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KKind {
    /// A proper (non-module, non-signature) type value with no finer nominal family —
    /// `List`, `Record`, `KFunction`, a bare resolved leaf, etc. As a slot it is the
    /// lowest-specificity proper-type slot (ties with `Identifier` in
    /// `is_more_specific_than`); it subsumes the three nominal families below it.
    ProperType,
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
    /// A slot accepting any (proper) type value (the `:Type` surface). Like `ProperType` it does
    /// not admit module / signature values — bare `Type` denotes "any type value", and the
    /// design pins the module/sig seam to the narrower `:Module` / `:Signature` slots. More
    /// specific than `ProperType` for tie-breaking; subsumes the proper subtree.
    AnyType,
    /// A newtype (record-repr or scalar) — the family a `NEWTYPE` or a user-`UNION` variant
    /// declares. Strictly below `ProperType`.
    NewType,
    /// A higher-kinded type constructor (`Result`). Strictly below `ProperType`.
    TypeConstructor,
    // Constructor(arity) — the `* -> *` arity tower for higher-kinded type constructors —
    // is the deferred extension point for higher-kinded type-constructor work. This enum
    // ships the shallow kinds only.
}

impl KKind {
    /// Reflexive subsumption: does a slot of kind `self` admit a type value classified as
    /// `other`? `ProperType` / `AnyType` admit the whole proper subtree; every other kind admits
    /// only itself — the module / signature wall keeps `:Type` from admitting a module, and a
    /// nominal-family slot admits only its own family.
    pub fn admits(self, other: KKind) -> bool {
        use KKind::*;
        match self {
            ProperType | AnyType => {
                matches!(other, ProperType | NewType | TypeConstructor)
            }
            Module => other == Module,
            Signature => other == Signature,
            NewType => other == NewType,
            TypeConstructor => other == TypeConstructor,
        }
    }

    /// Strict subsumption for specificity: `self` is a strictly-narrower kind than `other` in
    /// the lattice. The nominal families sit strictly below `ProperType`, so an
    /// `OfKind(NewType)` slot out-specifies an `OfKind(ProperType)` sibling.
    pub fn strictly_below(self, other: KKind) -> bool {
        use KKind::*;
        matches!((self, other), (NewType | TypeConstructor, ProperType))
    }

    /// Surface keyword rendered in diagnostics and type-name printing.
    pub fn surface_keyword(self) -> &'static str {
        match self {
            KKind::ProperType => "ProperType",
            KKind::Module => "Module",
            KKind::Signature => "Signature",
            KKind::AnyType => "Type",
            KKind::NewType => "NewType",
            KKind::TypeConstructor => "TypeConstructor",
        }
    }
}
