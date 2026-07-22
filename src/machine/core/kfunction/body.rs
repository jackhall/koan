//! Body-shape types: the return-contract carrier, the binder-hook `fn`-pointer aliases, the
//! body-statement splitters, and the `Body` enum (an action `fn` pointer vs a captured
//! user-defined `KExpression`).

use std::rc::Rc;

use crate::machine::model::{ExpressionPart, KExpression};

use crate::machine::core::{FrameStorage, RegionBrand, Scope};
use crate::machine::model::KType;
use crate::scheduler::Sealed;
use crate::witnessed::reattachable;

use super::KFunction;

/// Return-type contract a tail-replace carries to its Done arm, for both the
/// declared-return check and the error-frame label. A function-less return-typed tail (a
/// MATCH / TRY arm with `-> :T`) rides the same channel as an FN call: `Arm` carries the
/// declared type directly, `Function` reads it off the callee's signature.
///
/// `Arm`'s / `PerCall`'s `ret` is region-borrowed so the whole contract stays `Copy`, matching the
/// `&KFunction` it sits beside. Sealed as [`SealedContract`] into a `ReturnObligation` that rides the
/// tail chain as a continuation capture, pinned by its own carried witness. A tail chain keeps the
/// **first** contract (the keep-first rule at the `Outcome::Continue` construction sites in
/// `execute::runtime`, which wraps each replacement continuation with the established obligation), so
/// the check fires against the original caller's declared return, not the tail-most callee's.
#[derive(Clone, Copy)]
pub enum ReturnContract<'a> {
    /// An FN / builtin call: check against `signature.return_type`, label via `summarize()`.
    Function(&'a KFunction<'a>),
    /// A MATCH / TRY arm's `-> :T`: check the lifted value against `ret`, label with `kind`.
    /// `scope` is the arm's declaring scope — the call-site (outer) scope, a strict ancestor of the
    /// arm frame — so a coarsened re-tag re-homes there with no step-scope walk. `ret` is a `Copy`
    /// handle, so the contract stays `Copy`; [`Self::home_owner`] resolves the owning
    /// `Rc<FrameStorage>` off `scope` for the contract's carried witness.
    Arm {
        ret: KType,
        kind: &'static str,
        scope: &'a Scope<'a>,
    },
    /// A deferred-return FN whose per-call return type resolved to `ret`. Rides the FN-body
    /// chain shape (a `Function`/`PerCall` contract) so a tail-replaced deferred body assembles its
    /// lexical chain like any FN — preserving TCO — while `finalize_terminal` checks the
    /// lifted value against the resolved `ret` (labelled "per-call return type", `func` names
    /// the frame). `ret` is a `Copy` handle like `Arm`'s, so the contract stays `Copy`.
    PerCall { func: &'a KFunction<'a>, ret: KType },
}

impl<'a> ReturnContract<'a> {
    /// The contract's home region — where a coarsened re-tag is re-homed so it outlives the
    /// producer frame. A `Function`/`PerCall` reads it off the callee's captured-scope region; an
    /// `Arm` reads it off its declaring scope. All three are a strict ancestor region of the
    /// producing frame, so a re-tag there outlives it.
    pub fn home_region(self) -> RegionBrand<'a> {
        match self {
            ReturnContract::Function(f) | ReturnContract::PerCall { func: f, .. } => {
                f.captured_scope().brand()
            }
            ReturnContract::Arm { scope, .. } => scope.brand(),
        }
    }

    /// The `Rc<FrameStorage>` owning the contract's home region — resolved uniformly across every
    /// variant so the contract's own carried witness (not the cart's `outer` chain) pins it across a
    /// tail chain. `None` only when the owner's `Weak` has already released.
    pub fn home_owner(self) -> Option<Rc<FrameStorage>> {
        match self {
            ReturnContract::Function(f) | ReturnContract::PerCall { func: f, .. } => {
                f.captured_scope().region_owner().upgrade()
            }
            ReturnContract::Arm { scope, .. } => scope.region_owner().upgrade(),
        }
    }
}

/// `Reattachable` family for [`ReturnContract`] — the return-contract erasure carried on a node's
/// `TraceFrame`. Layout-invariant: the contract's arms are `&'a` references, a `&'static str`, and
/// a lifetime-free `KType` handle, whose representation does not depend on `'a`.
pub struct ContractFamily;

// `ReturnContract<'r>` is one type generic only in `'r` (every arm is a reference), layout identical
// for all `'r`; the shared `reattachable!` macro discharges that obligation once.
reattachable!(ContractFamily => ReturnContract<'r>);

/// A [`ReturnContract`] sealed into its dormant, `'static`-storage form for a `ReturnObligation`
/// riding a tail chain as a continuation capture. Pinned by its own carried witness —
/// [`ReturnContract::home_owner`]'s
/// `Rc<FrameStorage>`, folded into a [`FrameSet`](crate::machine::FrameSet) singleton at seal time
/// (a genuine pinning witness; the reference-only value carrier pins nothing) — not by the cart's
/// `outer` chain, so the contract's home region stays live across every hop of a tail chain
/// independent of which cart the slot currently carries. Re-anchored at the Done read boundary at
/// the step's combined open; the `Function` / `Arm` discriminant is readable there without
/// re-anchoring the pointee, for the chain-shape decision that needs the tag but not the pointee.
pub type SealedContract = Sealed<ContractFamily, crate::machine::FrameSet>;

/// Split an FN / MATCH-arm / TRY-arm body into top-level statements. The single source of
/// truth for the all-`Expression` multi-statement detection: any non-`Expression` part or
/// fewer than two parts leaves the body as a single statement. Always returns at least one
/// element. The runtime's `InScope` body fan-out (`KoanRuntime::apply_outcome`) routes through
/// here before `enter_block`, so the scheduler never inspects AST shape itself.
pub(crate) fn split_body_statements<'a>(body: KExpression<'a>) -> Vec<KExpression<'a>> {
    if body.is_statement_block() {
        body.parts
            .into_iter()
            .filter_map(|p| match p.value {
                ExpressionPart::Expression(e) => Some(*e),
                _ => None,
            })
            .collect()
    } else {
        vec![body]
    }
}

/// Borrowing twin of [`split_body_statements`]: returns references to the body's top-level
/// statements rather than owned clones, so the body AST is never duplicated on the call path. Same
/// multi-statement detection. The borrow lifetime is independent of the expression's own `'a`, so a
/// caller holding the body by value can scan it in place (`GROUP` reads its members off the
/// unevaluated body block this way).
pub(crate) fn body_statement_refs<'ast, 'a>(
    body: &'ast KExpression<'a>,
) -> Vec<&'ast KExpression<'a>> {
    if body.is_statement_block() {
        body.parts
            .iter()
            .filter_map(|p| match &p.value {
                ExpressionPart::Expression(e) => Some(e.as_ref()),
                _ => None,
            })
            .collect()
    } else {
        vec![body]
    }
}

pub use crate::machine::model::{BinderBucketFn, BinderNameFn};

/// Enum (not `Box<dyn Fn>`) so `UserDefined` stays introspectable — TCO and
/// error-frame attribution walk into the captured expression.
pub enum Body<'a> {
    UserDefined(KExpression<'a>),
    /// A builtin authored against the `Action` harness. Runs through
    /// `machine::execute::runtime::run_action`.
    Builtin(super::action::ActionFn),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the parser invariant [`split_body_statements`]'s `len() >= 2` guard relies on: a
    /// real, parser-produced body is never a lone `[Expression(_)]`. That shape is the one case
    /// where `len() >= 2` would treat a body differently from an `!is_empty()` guard, so were it
    /// reachable the guard would mis-split a single-statement body. It is unreachable because
    /// `peel_redundant` collapses redundant parens at every nesting level, so a body captured as
    /// a `(...)` argument arrives already peeled. If a parser change ever lets a real body surface
    /// as a lone `[Expression(_)]`, this fails.
    #[test]
    fn parser_never_yields_lone_expression_body() {
        use crate::parse::parse;

        // Each input captures a body as the trailing `(...)` argument of `FOO`; we extract that
        // body (the inner of the trailing `Expression` part) and assert it is never a lone
        // `[Expression(_)]`. Covers single-statement, multi-token, and genuine multi-statement
        // forms, each with a redundant outer paren the peeler must strip.
        for src in [
            "FOO ((a))",
            "FOO ((a b))",
            "FOO ((a)(b))",
            "FOO (a)",
            "FOO ((a) (b) (c))",
        ] {
            let body = parse(src)
                .expect("parse")
                .into_iter()
                .next()
                .expect("one statement")
                .parts
                .into_iter()
                .find_map(|p| match p.value {
                    ExpressionPart::Expression(e) => Some(*e),
                    _ => None,
                })
                .expect("captured body");
            let lone_expression = body.parts.len() == 1
                && matches!(body.parts[0].value, ExpressionPart::Expression(_));
            assert!(
                !lone_expression,
                "parser produced a lone [Expression(_)] body for {src:?}; the split fork has reopened"
            );
        }
    }
}
