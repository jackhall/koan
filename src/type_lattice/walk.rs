//! The two drivers every structural recursion in the lattice goes through.
//!
//! [`unary`] owns the arm table — which children a compound node has, in which order, and the
//! registry door that puts one back together — behind two entry points: [`unary::rebuild`] takes a
//! leaf rule and re-interns each rebuilt composite, [`unary::visit`] takes a pre-order rule that
//! descends, skips or stops. [`binary`] owns the pairing policy — positional, by name, or set-wise
//! — the variance flip, the width verdict per arm, and the mismatch case, behind one
//! [`binary::lockstep`] entry point whose instances supply an entry guard, a leaf verdict, a
//! set-wise rule and a structural combine.
//!
//! Rendering is the one recursion written by hand: it spells syntax *between* children and
//! inherits the quantifier binder from above, which neither driver expresses. Adding a compound
//! variant is a compile error at [`unary`]'s `children` / `reassemble` pair, at [`binary`]'s
//! pairing table, at the descent-knob sites, and in the rendering match — nowhere else.
//!
//! # Writing a new walk
//!
//! Ask first whether the walk is unary or binary, then whether it rebuilds.
//!
//! - A **unary rebuild** supplies a leaf rule `FnMut(KType, &TypeNode, &Context) -> Option<KType>`:
//!   `Some(k)` replaces the node and stops there, `None` lets the driver descend. Pick the descent
//!   knobs — whether a nested `Signature` is descended or treated as a leaf, and which union door
//!   reassembles a union. Substitution, binder canonicalization and sibling rewriting are all this.
//! - A **unary visit** supplies `FnMut(KType, &TypeNode, &Context) -> Visit` and reads the
//!   context. Occurrence censuses and reference folds are this.
//! - A **binary walk** implements [`binary::Lockstep`]. The order, the meet and the unifier's
//!   collector are its three instances.
//!
//! The context a unary rule is handed answers three questions the arm table alone cannot: whether
//! an enclosing descended signature declares a given abstract member, how many shape binders lie
//! between the root and here, and the variance of the current position.

pub mod binary;
pub mod unary;

/// Which way a position is being filled. A callable's parameter position flips it: the value there
/// must be *more general* than the position promises, its return *more specific*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variance {
    Co,
    Contra,
}

impl Variance {
    /// The variance under a parameter position — the one place the direction turns over.
    pub fn flipped(self) -> Self {
        match self {
            Variance::Co => Variance::Contra,
            Variance::Contra => Variance::Co,
        }
    }
}
