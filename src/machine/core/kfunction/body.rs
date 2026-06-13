//! Body-shape types: `BodyResult`, the binder-hook `fn`-pointer aliases, and the `Body` enum
//! (an action `fn` pointer vs a captured user-defined `KExpression`).

use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::{CallArena, KError, ScopeId};
use crate::machine::model::types::UntypedKey;
use crate::machine::model::values::{Carried, KObject};
use crate::machine::model::KType;

use super::scheduler_handle::NodeId;
use super::KFunction;

/// What a builtin's body returns.
///
/// `Tail { frame: Some(f), .. }` installs `f` as the slot's per-call frame and rewrites
/// its work to `Dispatch(expr)`, so a chain of tail calls reuses one slot. The previous
/// frame is dropped immediately; when the new child scope's `outer` IS the call site
/// (MATCH), `f` must hold the prior `Rc` via `CallArena::outer_frame` to keep
/// still-referenced memory alive. User-fn invokes are safe because the new child's
/// `outer` is the FN's captured scope, not the prior frame's.
///
/// `Tail { frame: None, .. }` keeps the slot's existing frame and scope.
///
/// `DeferTo(id)` rewrites the slot's work to `Lift { from: id }`. Used by binder bodies
/// (MODULE, SIG) that schedule a `Combine` to finalize their body statements: the
/// binder's slot lifts its terminal off the Combine.
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
    Arm {
        ret: &'a KType<'a>,
        kind: &'static str,
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

/// A [`ReturnContract`] with its lifetime erased to `'static` for storage on a lifetime-free
/// node `CallFrame`. The contract's `&KFunction` / `&KType` point into the cart's frame *outer*
/// arena (a strict ancestor — see `branch_walk::resolve_arm_return_contract` and
/// `invoke`'s `tail_with_frame_contract`), which the co-stored `cart: Rc<CallArena>` keeps live via its
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

pub enum BodyResult<'a> {
    /// A produced value in the scheduler's two-arm currency: a runtime [`KObject`] or a
    /// type flowing raw. Use [`BodyResult::value`] to wrap an object.
    Value(Carried<'a>),
    Tail {
        expr: KExpression<'a>,
        frame: Option<Rc<CallArena>>,
        /// Return contract the slot's Done arm enforces, and the source of the `TraceFrame`
        /// label appended to errors. `Function` is an FN / builtin call (checks
        /// `signature.return_type`, labels with `summarize()`); `Arm` is a MATCH / TRY
        /// arm whose `-> :T` declares a return type with no backing `&KFunction`. `None`
        /// for builtin tails that are deferred-eval continuations, not calls.
        function: Option<ReturnContract<'a>>,
        /// `Some(id)` means the tail enters a fresh lexical block (MATCH / TRY arms,
        /// FN body resolved-return); `None` continues the slot's current block (CONS
        /// tail, builtin tail continuations). The reinstall site
        /// (`compute_replace_chain` in `execute/scheduler/execute.rs`) prepends
        /// `(id, body_index)` for a non-`Function` contract (`Arm` or `None`); a
        /// `Function` contract assembles the chain via
        /// `kfunction/invoke.rs::assemble_body_chain`.
        block_entry: Option<ScopeId>,
        /// Body-scope chain index for a block-entry tail-replace. `1` for
        /// single-statement bodies — the lone statement sits above the `idx 0`
        /// parameters / `it`, so the strict `b.idx < c` predicate admits them. For
        /// multi-statement bodies tail-replacing into the *last* statement, this is
        /// `N`, admitting both the params and the `1..N-1` siblings already submitted
        /// against the body / arm scope. Ignored when `block_entry: None`.
        body_index: usize,
    },
    DeferTo(NodeId),
    Err(KError),
}

impl<'a> BodyResult<'a> {
    pub fn tail(expr: KExpression<'a>) -> Self {
        BodyResult::Tail {
            expr,
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        }
    }

    /// FN-body tail-replace carrying an explicit `contract` and `body_index` (see
    /// [`BodyResult::Tail`]). The `Function` form is the resolved-return call; the `PerCall` form
    /// is a deferred-return FN whose per-call type has been resolved. Both ride the FN-body chain
    /// shape (a `Function`/`PerCall` contract), so a deferred body tail-replaces like any FN and stays
    /// TCO-flat. `body_index` is `1` for a single-statement body (the lone statement sits above the
    /// `idx 0` params), or `N` when tail-replacing into the last of N statements.
    pub fn tail_with_frame_contract(
        expr: KExpression<'a>,
        frame: Rc<CallArena>,
        contract: ReturnContract<'a>,
        body_index: usize,
    ) -> Self {
        // Capture the scope id before `frame` moves into the variant; the reinstall
        // site reads it off `frame.scope()` to assemble the chain.
        let body_scope_id = frame.scope().id;
        BodyResult::Tail {
            expr,
            frame: Some(frame),
            function: Some(contract),
            block_entry: Some(body_scope_id),
            body_index,
        }
    }

    /// Block-entry tail-replace for builtins without a `&KFunction` (MATCH / TRY arms).
    /// `contract` is the arm's `-> :T` return contract, checked when its value lifts.
    pub fn tail_with_block(
        expr: KExpression<'a>,
        frame: Option<Rc<CallArena>>,
        scope_id: ScopeId,
        contract: Option<ReturnContract<'a>>,
    ) -> Self {
        Self::tail_with_block_at_index(expr, frame, scope_id, 1, contract)
    }

    /// Block-entry tail-replace with an explicit `body_index` (see [`BodyResult::Tail`]).
    pub fn tail_with_block_at_index(
        expr: KExpression<'a>,
        frame: Option<Rc<CallArena>>,
        scope_id: ScopeId,
        body_index: usize,
        contract: Option<ReturnContract<'a>>,
    ) -> Self {
        BodyResult::Tail {
            expr,
            frame,
            function: contract,
            block_entry: Some(scope_id),
            body_index,
        }
    }

    pub fn err(e: KError) -> Self {
        BodyResult::Err(e)
    }

    /// Wrap a runtime object as the `Object` arm of the value currency.
    pub fn value(o: &'a KObject<'a>) -> Self {
        BodyResult::Value(Carried::Object(o))
    }

    /// Wrap a type as the `Type` arm of the value currency — a type-operator's result rides
    /// the type channel raw (no `KObject` box). Pair with `scope.arena.alloc_ktype`.
    pub fn ktype(t: &'a KType<'a>) -> Self {
        BodyResult::Value(Carried::Type(t))
    }

    /// Test helper for bodies that contractually yield only `Value` or `Err`:
    /// extracts the `Value` payload, panicking with `ctx` otherwise.
    #[cfg(test)]
    pub fn expect_value(self, ctx: &str) -> &'a KObject<'a> {
        match self {
            BodyResult::Value(c) => c
                .as_object()
                .unwrap_or_else(|| panic!("{ctx}: expected Object value, got Type")),
            BodyResult::Tail { .. } => panic!("{ctx}: expected Value, got Tail"),
            BodyResult::DeferTo(_) => panic!("{ctx}: expected Value, got DeferTo"),
            BodyResult::Err(e) => panic!("{ctx}: expected Value, got Err({e})"),
        }
    }
}

/// Split an FN / MATCH-arm / TRY-arm body into top-level statements. Mirrors the
/// all-`Expression` detection used by
/// [`super::scheduler_handle::SchedulerHandle::enter_body_block`]; any non-`Expression`
/// part or fewer than two parts leaves the body as a single statement. Always returns
/// at least one element.
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
    /// `machine::execute::harness::run_action`.
    Builtin(super::action::ActionFn),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{KError, KErrorKind};
    use crate::machine::model::ast::KExpression;

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
        let erased = ErasedContract::erase(ReturnContract::Arm { ret, kind: "MATCH" });
        // Reattach witnessed by the cart `Rc`, then read through the re-anchored borrow.
        let reattached: ReturnContract<'_> = unsafe { erased.reattach(&cart) };
        match reattached {
            ReturnContract::Arm { ret, kind } => {
                assert!(matches!(ret, KType::Str));
                assert_eq!(kind, "MATCH");
            }
            ReturnContract::Function(_) | ReturnContract::PerCall { .. } => panic!("expected Arm"),
        }
    }

    #[test]
    fn err_constructor_wraps_kerror() {
        let kerr = KError::new(KErrorKind::MissingArg("x".into()));
        let result = BodyResult::<'_>::err(kerr);
        match result {
            BodyResult::Err(e) => match e.kind {
                KErrorKind::MissingArg(name) => assert_eq!(name, "x"),
                other => panic!(
                    "expected MissingArg, got {:?}",
                    std::mem::discriminant(&other)
                ),
            },
            _ => panic!("expected BodyResult::Err"),
        }
    }

    #[test]
    #[should_panic(expected = "ctx-tail: expected Value, got Tail")]
    fn expect_value_panics_on_tail() {
        let tail: BodyResult<'_> = BodyResult::tail(KExpression::new(Vec::new()));
        let _ = tail.expect_value("ctx-tail");
    }

    #[test]
    #[should_panic(expected = "ctx-defer: expected Value, got DeferTo")]
    fn expect_value_panics_on_defer_to() {
        let defer: BodyResult<'_> = BodyResult::DeferTo(NodeId(0));
        let _ = defer.expect_value("ctx-defer");
    }

    #[test]
    #[should_panic(expected = "ctx-err: expected Value, got Err(missing argument 'y')")]
    fn expect_value_panics_on_err() {
        let err: BodyResult<'_> = BodyResult::err(KError::new(KErrorKind::MissingArg("y".into())));
        let _ = err.expect_value("ctx-err");
    }
}
