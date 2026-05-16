//! Body-shape types: what a builtin body returns (`BodyResult`), the `fn`-pointer
//! aliases for builtin bodies and pre-run name hooks (`BuiltinFn`, `PreRunFn`), and the
//! `Body` enum that picks between a builtin pointer and a captured user-defined
//! `KExpression`.

use std::rc::Rc;

use crate::machine::model::ast::KExpression;

use crate::machine::core::{CallArena, KError, Scope};
use crate::machine::model::values::KObject;

use super::argument_bundle::ArgumentBundle;
use super::scheduler_handle::{NodeId, SchedulerHandle};
use super::KFunction;

/// What a builtin's body returns.
///
/// `Tail { expr, frame: Some(f), .. }` installs the per-call `CallArena` `f` in the slot;
/// the scheduler rewrites the slot's work to `Dispatch(expr)` and re-runs it, so a chain of
/// tail calls reuses one slot. A TCO replace drops the slot's previous frame immediately;
/// for user-fn invokes that's safe (the new child scope's `outer` is the FN's captured
/// scope, not the previous frame's), and for builtins whose new child scope's `outer` IS
/// the call site (MATCH), the new frame holds the previous frame's `Rc` via
/// `CallArena::outer_frame` so the drop doesn't free memory still in use.
///
/// `Tail { frame: None, .. }` keeps the slot's existing frame and scope.
///
/// `DeferTo(id)` rewrites the slot's work to `Lift { from: id }`; the slot's terminal
/// becomes whatever `id` produces. Used by binder bodies (MODULE, SIG) that schedule a
/// `Combine` to wrap up their body statements: the Combine owns the finalize work and the
/// binder's own slot lifts its terminal off the Combine. Same shape as `defer_to_lift`'s
/// post-Bind park, exposed to bodies for combinator-style planning.
pub enum BodyResult<'a> {
    Value(&'a KObject<'a>),
    Tail {
        expr: KExpression<'a>,
        frame: Option<Rc<CallArena>>,
        /// User-fn reference attached to the slot for two purposes: the slot's Done arm
        /// reads `signature.return_type` to enforce the declared return type at runtime,
        /// and on error `function.summarize()` becomes the appended `Frame`'s function
        /// name. `None` for builtin tails that are deferred-eval continuations, not calls.
        function: Option<&'a KFunction<'a>>,
    },
    DeferTo(NodeId),
    Err(KError),
}

impl<'a> BodyResult<'a> {
    pub fn tail(expr: KExpression<'a>) -> Self {
        BodyResult::Tail { expr, frame: None, function: None }
    }

    pub fn tail_with_frame(
        expr: KExpression<'a>,
        frame: Rc<CallArena>,
        function: &'a KFunction<'a>,
    ) -> Self {
        BodyResult::Tail { expr, frame: Some(frame), function: Some(function) }
    }

    pub fn err(e: KError) -> Self {
        BodyResult::Err(e)
    }

    /// Test helper for bodies that contractually only yield `Value` or `Err`:
    /// extracts the `Value` payload, panicking with `ctx` and the actual
    /// variant name otherwise. Collapses the 4-arm match pattern that used to
    /// be repeated across builtin unit tests.
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

/// Builtin body. `for<'a>` so a single `fn` works for any caller scope lifetime;
/// `Scope` is `&'a` (not `&mut`) because every node spawned during the body shares it
/// — mutability is interior via `RefCell`.
pub type BuiltinFn = for<'a> fn(
    &'a Scope<'a>,
    &mut dyn SchedulerHandle<'a>,
    ArgumentBundle<'a>,
) -> BodyResult<'a>;

/// Dispatch-time name extractor for a binder builtin. `run_dispatch` calls it on the
/// unresolved expression *before* sub-deps are scheduled; returning `Some(name)` installs
/// `placeholders[name] = NodeId(this_slot)` in the dispatching scope so a sibling looking
/// up `name` while this slot's body is still in flight parks on this slot (see
/// [`crate::machine::core::Scope::resolve`]). `None` opts out.
pub type PreRunFn = for<'a> fn(&KExpression<'a>) -> Option<String>;

/// An enum (rather than `Box<dyn Fn>`) so the `UserDefined` case stays introspectable —
/// TCO and error-frame attribution both need to walk into the captured expression.
pub enum Body<'a> {
    Builtin(BuiltinFn),
    UserDefined(KExpression<'a>),
}
