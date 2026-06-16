//! Body-shape types: the return-contract carrier, the binder-hook `fn`-pointer aliases, the
//! body-statement splitters, and the `Body` enum (an action `fn` pointer vs a captured
//! user-defined `KExpression`).

use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::{CallArena, RuntimeArena};
use crate::machine::model::types::UntypedKey;
use crate::machine::model::KType;

use super::KFunction;

/// Return-type contract a tail-replace carries to its Done arm, for both the
/// declared-return check and the error-frame label. A function-less return-typed tail (a
/// MATCH / TRY arm with `-> :T`) rides the same channel as an FN call: `Arm` carries the
/// declared type directly, `Function` reads it off the callee's signature.
///
/// `Arm`'s / `PerCall`'s `ret` is arena-borrowed so the whole contract stays `Copy`, matching the
/// `&KFunction` it sits beside. Stored erased as [`ErasedContract`] on the node's `TraceFrame`. A tail
/// chain keeps the **first** contract (the `next_contract` rule in `execute::scheduler::execute`),
/// so the check fires against the original caller's declared return, not the tail-most callee's.
#[derive(Clone, Copy)]
pub enum ReturnContract<'a> {
    /// An FN / builtin call: check against `signature.return_type`, label via `summarize()`.
    Function(&'a KFunction<'a>),
    /// A MATCH / TRY arm's `-> :T`: check the lifted value against `ret`, label with `kind`.
    /// `arena` is the arm's home arena — the call-site (outer) arena `ret` is allocated in, a
    /// strict ancestor of the arm frame — so a coarsened re-tag re-homes there with no step-scope
    /// walk. `&RuntimeArena` is `Copy`, so the contract stays `Copy`; the cart `Rc` witnesses it.
    Arm {
        ret: &'a KType<'a>,
        kind: &'static str,
        arena: &'a RuntimeArena,
    },
    /// A deferred-return FN whose per-call return type resolved to `ret`. Rides the FN-body
    /// chain shape (a `Function`/`PerCall` contract) so a tail-replaced deferred body assembles its
    /// lexical chain like any FN — preserving TCO — while `check_declared_return` checks the
    /// lifted value against the resolved `ret` (labelled "per-call return type", `func` names
    /// the frame). `ret` is arena-borrowed like `Arm`'s, so the contract stays `Copy`.
    PerCall {
        func: &'a KFunction<'a>,
        ret: &'a KType<'a>,
    },
}

impl<'a> ReturnContract<'a> {
    /// The contract's home arena — where a coarsened re-tag is re-homed so it outlives the
    /// producer frame. A `Function`/`PerCall` reads it off the callee's captured-scope arena; an
    /// `Arm` carries it directly. All three are the cart's *outer* (ancestor) arena, witnessed by
    /// the cart `Rc`, so the Done boundary derives it from the contract with no scope walk.
    pub fn home_arena(self) -> &'a RuntimeArena {
        match self {
            ReturnContract::Function(f) | ReturnContract::PerCall { func: f, .. } => {
                f.captured_scope().arena
            }
            ReturnContract::Arm { arena, .. } => arena,
        }
    }
}

/// A [`ReturnContract`] with its lifetime erased to `'static` for storage on a lifetime-free
/// node `CallFrame`. The contract's `&KFunction` / `&KType` point into the cart's frame *outer*
/// arena (a strict ancestor — see `branch_walk::resolve_arm_return_contract` and `invoke`'s
/// `Outcome::Continue` tail construction), which the co-stored `cart: Rc<CallArena>` keeps live via its
/// `outer_frame` / escape chain. So the cart is the liveness witness: while it is held, the
/// contract's home arena cannot drop.
///
/// This is the single audited owner of the contract erasure, mirroring
/// [`ScopePtr`](crate::machine::core::scope_ptr::ScopePtr): the lifetime is forgotten for
/// storage and re-anchored at the Done read boundary, witnessed by the cart. The `Function` /
/// `Arm` discriminant is readable without a re-anchor for the chain-shape decision that needs the
/// tag but not the pointee.
#[derive(Clone, Copy)]
pub struct ErasedContract {
    inner: ReturnContract<'static>,
}

impl ErasedContract {
    /// Erase a live contract to its storable `'static` form. Safe: forgetting a lifetime for
    /// storage cannot fabricate one — the value is never *used* at `'static`, only stored, and
    /// [`Self::reattach`] shortens it back to a cart-witnessed lifetime before any use.
    pub fn erase(contract: ReturnContract<'_>) -> Self {
        // SAFETY: `ReturnContract<'a>` and `ReturnContract<'static>` share layout (a lifetime
        // never changes representation); the erased value is stored, not dereferenced, until
        // `reattach` re-anchors it.
        ErasedContract {
            inner: unsafe {
                std::mem::transmute::<ReturnContract<'_>, ReturnContract<'static>>(contract)
            },
        }
    }

    /// Re-anchor the contract to a caller-chosen `'a`, witnessed by the cart `Rc` co-stored with
    /// it on the node's `TraceFrame`. The single fabrication for this carrier — mirrors
    /// [`CallArena::scope`](crate::machine::core::CallArena::scope)'s unbounded re-attach.
    ///
    /// SAFETY: `_witness` is the cart that pins the contract's home arena (a strict ancestor of
    /// the cart's own frame) for as long as it is held. The caller re-anchors only at the Done
    /// boundary, holding the cart across the use, so the returned `'a` borrow cannot outlive the
    /// pointee. `'a` is driven by the return-type annotation (late-bound, like
    /// `reattach_unbounded`), not a turbofish argument.
    pub unsafe fn reattach<'a>(self, _witness: &Rc<CallArena>) -> ReturnContract<'a> {
        std::mem::transmute::<ReturnContract<'static>, ReturnContract<'a>>(self.inner)
    }
}

/// Split an FN / MATCH-arm / TRY-arm body into top-level statements. The single source of
/// truth for the all-`Expression` multi-statement detection: any non-`Expression` part or
/// fewer than two parts leaves the body as a single statement. Always returns at least one
/// element. The runtime's `InScope` body fan-out (`KoanRuntime::apply_outcome`) routes through
/// here before `enter_block`, so the scheduler never inspects AST shape itself.
pub(crate) fn split_body_statements<'a>(body: KExpression<'a>) -> Vec<KExpression<'a>> {
    let is_multi = body.parts.len() >= 2
        && body
            .parts
            .iter()
            .all(|p| matches!(p.value, ExpressionPart::Expression(_)));
    if is_multi {
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
/// multi-statement detection.
pub(crate) fn body_statement_refs<'ast>(
    body: &'ast KExpression<'ast>,
) -> Vec<&'ast KExpression<'ast>> {
    let is_multi = body.parts.len() >= 2
        && body
            .parts
            .iter()
            .all(|p| matches!(p.value, ExpressionPart::Expression(_)));
    if is_multi {
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

/// Dispatch-time name extractor for a binder builtin. Returning `Some(name)` installs
/// `placeholders[name] = NodeId(this_slot)` so a sibling looking up `name` while the
/// body is in flight parks on this slot (see [`crate::machine::core::Scope::resolve`]).
pub type BinderNameFn = for<'a> fn(&KExpression<'a>) -> Option<String>;

/// Dispatch-time bucket-key extractor for a binder that registers a callable
/// (`FN`, `FUNCTOR`). Returns the `UntypedKey` for a *call* to the to-be-registered
/// overload (e.g. `(MAKESET Er :OrderedSig)` → `[Keyword("MAKESET"), Slot]`); the
/// driver installs it in `bindings.pending_overloads` so a sibling call form parks
/// on the producer instead of failing dispatch.
///
/// Separate from [`BinderNameFn`] because the two key different resolvers:
/// `BinderNameFn` for `Scope::resolve`, `BinderBucketFn` for the no-bucket fallback
/// in `resolve_dispatch`. Keying on the full bucket (not just the lead keyword)
/// keeps overloads sharing a head keyword but differing in later keywords
/// (`MAKESET _` vs `MAKESET _ USING _`) from colliding on the park edge.
pub type BinderBucketFn = for<'a> fn(&KExpression<'a>) -> Option<UntypedKey>;

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

    /// Miri slate (tree borrows): the [`ErasedContract`] erase → reattach round-trip. `erase`
    /// forgets the contract's lifetime for storage; `reattach` transmutes it back to a lifetime
    /// witnessed by the cart `Rc` that pins the contract's home arena. Minimal-shape mirror of the
    /// transmute pair (body.rs) and its unbounded call site (execute.rs); fails on UB, not values.
    #[test]
    fn erased_contract_reattach_roundtrip() {
        use crate::builtins::default_scope;
        use crate::machine::core::RuntimeArena;

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let cart = CallArena::new(scope, None);
        // Stands in for a MATCH/TRY arm's `-> :T`, allocated in the cart's own arena.
        let ret: &KType = cart.arena().alloc_ktype(KType::Str);
        let erased = ErasedContract::erase(ReturnContract::Arm {
            ret,
            kind: "MATCH",
            arena: cart.arena(),
        });
        // Reattach witnessed by the cart `Rc`, then read through the re-anchored borrow.
        let reattached: ReturnContract<'_> = unsafe { erased.reattach(&cart) };
        match reattached {
            ReturnContract::Arm { ret, kind, .. } => {
                assert!(matches!(ret, KType::Str));
                assert_eq!(kind, "MATCH");
            }
            ReturnContract::Function(_) | ReturnContract::PerCall { .. } => panic!("expected Arm"),
        }
    }

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
