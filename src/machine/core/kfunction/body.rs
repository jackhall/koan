//! Body-shape types: `BodyResult`, the builtin-body / binder-hook `fn`-pointer aliases,
//! and the `Body` enum (builtin pointer vs captured `KExpression`).

use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

use crate::machine::core::{CallArena, KError, Scope, ScopeId};
use crate::machine::model::types::UntypedKey;
use crate::machine::model::values::KObject;
use crate::machine::model::KType;

use super::argument_bundle::ArgumentBundle;
use super::scheduler_handle::{NodeId, SchedulerHandle};
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
/// declared-return check and the error-frame label. Generalizes what was a bare
/// `&KFunction` carrier so a function-less return-typed tail (a MATCH / TRY arm with
/// `-> :T`) rides the same channel as an FN call.
///
/// `Arm`'s `ret` is arena-borrowed so the whole contract stays `Copy`, matching the
/// `&KFunction` it sits beside. Same TCO limitation as the `function` carrier
/// ([`crate::machine::execute`] `Node`): a nested tail-call rewrites the contract to
/// the callee, so the check fires only against the tail-most contract.
#[derive(Clone, Copy)]
pub enum ReturnContract<'a> {
    /// An FN / builtin call: check against `signature.return_type`, label via `summarize()`.
    Function(&'a KFunction<'a>),
    /// A MATCH / TRY arm's `-> :T`: check the lifted value against `ret`, label with `kind`.
    Arm {
        ret: &'a KType<'a>,
        kind: &'static str,
    },
}

pub enum BodyResult<'a> {
    Value(&'a KObject<'a>),
    Tail {
        expr: KExpression<'a>,
        frame: Option<Rc<CallArena>>,
        /// Return contract the slot's Done arm enforces, and the source of the `Frame`
        /// label appended to errors. `Function` is an FN / builtin call (checks
        /// `signature.return_type`, labels with `summarize()`); `Arm` is a MATCH / TRY
        /// arm whose `-> :T` declares a return type with no backing `&KFunction`. `None`
        /// for builtin tails that are deferred-eval continuations, not calls.
        function: Option<ReturnContract<'a>>,
        /// `Some(id)` means the tail enters a fresh lexical block (MATCH / TRY arms,
        /// FN body resolved-return); `None` continues the slot's current block (CONS
        /// tail, builtin tail continuations). The reinstall site
        /// (`compute_replace_chain` in `execute/scheduler/execute.rs`) prepends
        /// `(id, 0)` for a non-`Function` contract (`Arm` or `None`); a `Function`
        /// contract assembles the chain via `kfunction/invoke.rs::assemble_body_chain`.
        block_entry: Option<ScopeId>,
        /// Body-scope chain index for a block-entry tail-replace. `0` for
        /// single-statement bodies. For multi-statement bodies tail-replacing into
        /// the *last* statement, this is `N` so the strict `b.idx < c` predicate
        /// admits the `1..N-1` siblings already submitted against the body / arm
        /// scope. Ignored when `block_entry: None`.
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

    pub fn tail_with_frame(
        expr: KExpression<'a>,
        frame: Rc<CallArena>,
        function: &'a KFunction<'a>,
    ) -> Self {
        Self::tail_with_frame_at_index(expr, frame, function, 0)
    }

    /// FN-body tail-replace with an explicit `body_index` (see [`BodyResult::Tail`]).
    pub fn tail_with_frame_at_index(
        expr: KExpression<'a>,
        frame: Rc<CallArena>,
        function: &'a KFunction<'a>,
        body_index: usize,
    ) -> Self {
        // Capture the scope id before `frame` moves into the variant; the reinstall
        // site reads it off `frame.scope()` to assemble the chain.
        let body_scope_id = frame.scope().id;
        BodyResult::Tail {
            expr,
            frame: Some(frame),
            function: Some(ReturnContract::Function(function)),
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
        Self::tail_with_block_at_index(expr, frame, scope_id, 0, contract)
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

    /// Test helper for bodies that contractually yield only `Value` or `Err`:
    /// extracts the `Value` payload, panicking with `ctx` otherwise.
    #[cfg(test)]
    pub fn expect_value(self, ctx: &str) -> &'a KObject<'a> {
        match self {
            BodyResult::Value(v) => v,
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

/// Builtin body. `Scope` is `&'a` (not `&mut`) — every node spawned during the body
/// shares it; mutability is interior via `RefCell`.
pub type BuiltinFn =
    for<'a> fn(&'a Scope<'a>, &mut dyn SchedulerHandle<'a>, ArgumentBundle<'a>) -> BodyResult<'a>;

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
    Builtin(BuiltinFn),
    UserDefined(KExpression<'a>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{KError, KErrorKind};
    use crate::machine::model::ast::KExpression;

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
