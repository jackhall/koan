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
/// Any > { Module, Signature, Proper > { Tagged, Newtype, TypeConstructor } }
/// ```
///
/// `kind_of` (the value-classification direction) descends a nominal to its family —
/// [`Proper`](KKind::Proper) only for a non-nominal, non-module, non-signature type;
/// [`Module`](KKind::Module) / [`Signature`](KKind::Signature) for those carriers; and
/// [`Tagged`](KKind::Tagged) / [`Newtype`](KKind::Newtype) /
/// [`TypeConstructor`](KKind::TypeConstructor) for a user-declared nominal. [`Any`](KKind::Any)
/// is a *slot* expectation only ("accepts any proper type value"), never a classification.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KKind {
    /// A proper (non-module, non-signature) type value with no finer nominal family —
    /// `List`, `Record`, `KFunction`, a bare resolved leaf, etc. As a slot it is the
    /// lowest-specificity proper-type slot (ties with `Identifier` in
    /// `is_more_specific_than`); it subsumes the three nominal families below it.
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
    /// specific than `Proper` for tie-breaking; subsumes the proper subtree.
    Any,
    /// A tagged-union type — the family a user-`UNION` declares. Strictly below `Proper`.
    Tagged,
    /// A newtype (record-repr or scalar) — the family a `NEWTYPE` declares. Strictly below
    /// `Proper`.
    Newtype,
    /// A higher-kinded type constructor (`Result`). Strictly below `Proper`.
    TypeConstructor,
    // Constructor(arity) — the `* -> *` arity tower for higher-kinded type constructors —
    // is the deferred extension point for higher-kinded type-constructor work. This enum
    // ships the shallow kinds only.
}

impl KKind {
    /// Reflexive subsumption: does a slot of kind `self` admit a type value classified as
    /// `other`? `Proper` / `Any` admit the proper subtree (`Proper` itself and the three
    /// nominal families); every other kind admits only itself — the module / signature wall
    /// keeps `:Type` from admitting a module, and a nominal-family slot admits only its own
    /// family.
    pub fn admits(self, other: KKind) -> bool {
        use KKind::*;
        match self {
            Proper | Any => matches!(other, Proper | Tagged | Newtype | TypeConstructor),
            Module => other == Module,
            Signature => other == Signature,
            Tagged => other == Tagged,
            Newtype => other == Newtype,
            TypeConstructor => other == TypeConstructor,
        }
    }

    /// Strict subsumption for specificity: `self` is a strictly-narrower kind than `other` in
    /// the lattice. The three nominal families sit strictly below `Proper`, so an
    /// `OfKind(Tagged)` slot out-specifies an `OfKind(Proper)` sibling.
    pub fn strictly_below(self, other: KKind) -> bool {
        use KKind::*;
        matches!((self, other), (Tagged | Newtype | TypeConstructor, Proper))
    }

    /// Surface keyword rendered in diagnostics and type-name printing.
    pub fn surface_keyword(self) -> &'static str {
        match self {
            KKind::Proper => "TypeExprRef",
            KKind::Module => "Module",
            KKind::Signature => "Signature",
            KKind::Any => "Type",
            KKind::Tagged => "Tagged",
            KKind::Newtype => "Newtype",
            KKind::TypeConstructor => "TypeConstructor",
        }
    }
}
