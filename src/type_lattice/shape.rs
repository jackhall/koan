//! The vocabulary an expression shape is spelled in: the element run a call must match, the
//! surface a deferred return is shadowed as, and the four-verdict specificity a dispatch ranks
//! candidates by.
//!
//! A shape node itself lives in [`node`](super::node); this file owns the pieces that appear
//! *inside* one, plus the specificity verdict [`shape_specificity`](super::sig_relations)
//! produces.

use crate::parse::{KeywordSymbol, LabelInterner, TypeSymbol};

use super::handle::KType;

/// One position of an expression shape: a fixed token as its [`KeywordSymbol`], or an argument
/// slot carrying its declared type. Both arms are `Copy` handles — a symbol is `u128` bits, a
/// [`KType`] is interned in the run registry — so an element compares meaningfully wherever it is
/// stored and a re-homed run copies nothing but the elements themselves.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DispatchTokenElement {
    Keyword(KeywordSymbol),
    Slot(KType),
}

/// A confined FN `ret` slot whose source return is deferred to per-call elaboration — `-> er` or
/// `-> er.Carrier`. It holds only the hashable surface shadow: a bare name's lifetime-free
/// [`TypeSymbol`], or an expression's canonical render. Identity is syntactic, so a
/// [`TypeNode::DeferredReturn`](super::node::TypeNode::DeferredReturn) compares, hashes and
/// digests by surface form.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum DeferredReturnSurface {
    Type(TypeSymbol),
    Expression(String),
}

impl DeferredReturnSurface {
    /// Surface form for diagnostics, written straight into `f`; the `Type` carrier resolves its
    /// spelling through the run's interner and the `Expression` carrier writes the text it stores.
    pub fn write_surface(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        labels: &LabelInterner,
    ) -> std::fmt::Result {
        match self {
            Self::Type(name) => write!(f, "{}", labels.display(name.symbol())),
            Self::Expression(text) => f.write_str(text),
        }
    }

    /// Whether this surface opens with the type sigil — the
    /// [`surface_opens_sigil`](super::render::surface_opens_sigil) arm for a deferred return. A
    /// `Type` carrier names a bare token; an `Expression` carrier answers for the text it stores.
    pub fn opens_sigil(&self) -> bool {
        match self {
            Self::Type(_) => false,
            Self::Expression(text) => text.starts_with(':'),
        }
    }
}

/// How two candidates under one dispatch bucket rank against each other.
/// `Incomparable` means neither dominates: each admits an argument tuple the other rejects.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Specificity {
    StrictlyMore,
    StrictlyLess,
    Equal,
    Incomparable,
}
