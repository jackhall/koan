//! `KKind` — the shallow dispatch *kind* of a type. A type-accepting argument slot carries a kind
//! expectation as [`TypeNode::OfKind`](super::node::TypeNode::OfKind); a type value flowing into
//! such a slot is classified by [`KType::kind_of`](super::handle::KType::kind_of) and matched
//! against it. `OfKind` is **type-channel only** — it admits a type value, never a runtime
//! instance (a value is matched by a type, never by a kind).
//!
//! See [design/typing/type-lattice.md](../../design/typing/type-lattice.md).

use crate::parse::{LabelInterner, StaticName, TypeSymbol};

/// Shallow kind of a type, used to admit a type value into a type-accepting slot. The kinds
/// form one subsumption lattice:
///
/// ```text
/// AnyType > { Signature, ProperType > { NewType, TypeConstructor } }
/// ```
///
/// [`AnyType`](KKind::AnyType) is a *slot* expectation only ("accepts any proper type value"),
/// never a value classification produced by [`kind_of`](super::handle::KType::kind_of).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KKind {
    /// A proper (non-module, non-signature) type value with no finer nominal family —
    /// `List`, `Record`, `KFunction`, a bare resolved leaf, etc. It subsumes the nominal
    /// families below it.
    ProperType,
    /// A first-class signature value's kind, and the `:Signature` wildcard slot. The
    /// sig-as-constraint role is the separate [`TypeNode::Signature`](super::node::TypeNode::Signature),
    /// which is also where a module lands: a module is a *value*, matched by a signature type,
    /// and the `:Module` surface lowers to the empty signature rather than to a kind.
    Signature,
    /// A slot accepting any type value (the `:Type` surface) — the kind lattice's top: the proper
    /// subtree and signature values alike.
    AnyType,
    /// A newtype (record-repr or scalar) — the family a `NEWTYPE` or a user-`UNION` variant
    /// declares. Strictly below `ProperType`.
    NewType,
    /// A higher-kinded type constructor (`Result`). Strictly below `ProperType`.
    TypeConstructor,
}

impl KKind {
    /// Reflexive subsumption: does a slot of kind `self` admit a type value classified as
    /// `other`? `AnyType` admits every type value — a signature is a type value, so `:Type`
    /// takes it like any other. `ProperType` admits the proper subtree only — the signature
    /// wall lives here: a proper-type slot names what can type an ordinary value, which a
    /// signature is not. Every other kind admits only itself.
    ///
    /// This is the whole order on the type channel: `OfKind(x) ≤ OfKind(y)` iff `y.admits(x)`.
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

    /// Surface keyword rendered in diagnostics and type-name printing.
    pub fn surface_keyword(self) -> &'static str {
        self.surface_name().text()
    }

    /// The surface keyword's [`StaticName`] — the one spelling authority
    /// [`surface_keyword`](Self::surface_keyword) and [`surface_symbol`](Self::surface_symbol)
    /// both read, so the rendered name and the classified symbol cannot drift apart.
    fn surface_name(self) -> &'static StaticName<TypeSymbol> {
        static PROPER_TYPE: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "ProperType");
        static SIGNATURE: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Signature");
        static ANY_TYPE: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "Type");
        static NEW_TYPE: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "NewType");
        static TYPE_CONSTRUCTOR: StaticName<TypeSymbol> =
            crate::static_name!(TypeSymbol, "TypeConstructor");
        match self {
            KKind::ProperType => &PROPER_TYPE,
            KKind::Signature => &SIGNATURE,
            KKind::AnyType => &ANY_TYPE,
            KKind::NewType => &NEW_TYPE,
            KKind::TypeConstructor => &TYPE_CONSTRUCTOR,
        }
    }

    /// The surface keyword as its classified [`TypeSymbol`], minted once per process off the
    /// memo and recorded under `labels` so a diagnostic naming it can render the text.
    pub fn surface_symbol(self, labels: &LabelInterner) -> TypeSymbol {
        labels.record(self.surface_name())
    }
}
