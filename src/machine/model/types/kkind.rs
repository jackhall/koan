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
/// AnyType > { Signature, ProperType > { NewType, TypeConstructor } }
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
    /// A first-class signature value's kind, and the `:Signature` wildcard slot. The
    /// sig-as-constraint role is the separate [`KType::Signature`](super::ktype::KType::Signature),
    /// which is also where a module lands: a module is a *value*, matched by a signature type,
    /// and the `:Module` surface lowers to the empty signature rather than to a kind.
    Signature,
    /// A slot accepting any type value (the `:Type` surface) — the lattice top: the proper
    /// subtree and signature values alike. A signature is a type value, so bare `Type` admits
    /// it; the narrower `:Signature` slot out-specifies a `:Type` sibling for signature
    /// values. More specific than `ProperType` for tie-breaking.
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
    /// `other`? `AnyType` admits every type value — a signature is a type value, so `:Type`
    /// takes it like any other. `ProperType` admits the proper subtree only — the signature
    /// wall lives here: a proper-type slot (a parsed type-name declarator, a nominal head)
    /// names what can type an ordinary value, which a signature is not. Every other kind
    /// admits only itself.
    pub fn admits(self, other: KKind) -> bool {
        use KKind::*;
        match self {
            AnyType => true,
            ProperType => matches!(other, ProperType | NewType | TypeConstructor),
            Signature => other == Signature,
            NewType => other == NewType,
            TypeConstructor => other == TypeConstructor,
        }
    }

    /// Strict subsumption for specificity: `self` is a strictly-narrower kind than `other` in
    /// the lattice. The nominal families sit strictly below `ProperType`, so an
    /// `OfKind(NewType)` slot out-specifies an `OfKind(ProperType)` sibling; `Signature` sits
    /// strictly below `AnyType`, so a `:Signature` slot out-specifies a `:Type` sibling when
    /// both admit a signature value. (`ProperType` is ordered against `AnyType` by the
    /// unconstrained-name tier in `more_specific_walk`, not here.)
    pub fn strictly_below(self, other: KKind) -> bool {
        use KKind::*;
        matches!(
            (self, other),
            (NewType | TypeConstructor, ProperType) | (Signature, AnyType)
        )
    }

    /// Surface keyword rendered in diagnostics and type-name printing.
    pub fn surface_keyword(self) -> &'static str {
        match self {
            KKind::ProperType => "ProperType",
            KKind::Signature => "Signature",
            KKind::AnyType => "Type",
            KKind::NewType => "NewType",
            KKind::TypeConstructor => "TypeConstructor",
        }
    }
}
