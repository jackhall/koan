//! **Under construction, feature-gated (`exec-v2`).** The innermost layer of the parallel
//! body executor — pure koan semantics, no scheduler task format, no lifting.
//!
//! `exec` runs a body in its per-call frame and describes — in its *native* terms
//! ([`KExpression`], [`Carried`]) — what should happen next, as a [`Next`]: it failed, it
//! produced a (still-unlifted) value, it tail-calls after some fire-and-forget effects, or it
//! suspends awaiting some sub-expressions. It names *expressions to dispatch* and a
//! *continuation* — never a scheduler `Task`, never the scheduler itself.
//!
//! The lifetime-aware shell that turns a [`Next`] into scheduler-formatted opaque tasks (and
//! lifts the value) lives on the scheduler side — see `execute`'s exec adapter. Keeping that
//! out of here is what lets `exec` stay scheduler-agnostic and `'run`-free.
//!
//! ## One working lifetime
//!
//! `exec` operates entirely in the frame: the `'a` on [`Next`] is the body's working (frame)
//! lifetime — the unlifted value lives there, and the expressions it hands back are valid for
//! it. The adapter re-anchors those expressions to the scheduler's longer-lived AST lifetime
//! and lifts the value out of the frame; `exec` does neither (it holds no lift handle, so it
//! *cannot* lift).

use std::rc::Rc;

use crate::machine::core::{CallArena, KError, LexicalFrame, Scope};
use crate::machine::model::ast::KExpression;
use crate::machine::model::values::Carried;

use super::argument_bundle::ArgumentBundle;
use super::KFunction;

/// A body's execution context: the per-call `arena` it runs in, plus its lexical `chain`. Owned
/// (both fields are `Rc`), so it carries no lifetime; the body re-projects its scope from the
/// arena on demand. The arena rides forward via the `Rc` — no borrow is stored.
#[derive(Clone)]
pub struct Frame {
    /// The per-call arena the body executes in: it backs allocations, and its child scope is the
    /// body's scope. Owned — supplied (and, for TCO, reset) by the scheduler.
    pub arena: Rc<CallArena>,
    /// The body's lexical position — the parent chain for sub-expressions it hands back.
    pub chain: Rc<LexicalFrame>,
}

impl Frame {
    /// The scope where bindings land and effects accumulate. Re-projected from the owned arena,
    /// bounded by `&self`: a transient borrow that never escapes.
    pub fn scope(&self) -> &Scope<'_> {
        self.arena.scope_bounded()
    }
}

/// A joined dep's resolved terminal, handed to a continuation ([`Next::Suspend`]) on re-entry.
pub type DepResult<'a> = Result<Carried<'a>, KError>;

/// The continuation of a suspended body, in `exec`'s native terms: re-entered with its owned
/// [`Frame`] (so it can re-thread the frame) and the resolved join results, yielding
/// another [`Next`]. Borrow-free: frame-local state it needs is re-read by re-projecting
/// `ctx.scope()`, not captured. The scheduler-side adapter wraps this into a stored, opaque
/// scheduler continuation.
pub type Resume<'a> = Box<dyn FnOnce(Frame, &[DepResult<'a>]) -> Next<'a> + 'a>;

/// **exec → adapter.** What running a body describes next, in `exec`'s native currency
/// (`KExpression` + `Carried`). The scheduler-side adapter translates it into opaque tasks.
pub enum Next<'a> {
    /// The body failed; propagate the error.
    Errored(KError),
    /// The body produced its result — **still in the frame, unlifted.** The adapter/scheduler
    /// lifts it out; `exec` cannot.
    Value(Carried<'a>),
    /// Dispatch `effects` fire-and-forget (run for their `Scope` effects, results ignored), then
    /// tail into `tail` in the same frame — the multi-statement-body case.
    Tail {
        effects: Vec<KExpression<'a>>,
        tail: KExpression<'a>,
    },
    /// Suspend: dispatch and await `join`, then re-enter `resume` with their results.
    Suspend {
        join: Vec<KExpression<'a>>,
        resume: Resume<'a>,
    },
}

/// The new `invoke` for a user-defined function: bind `bundle`'s parameters into `ctx`'s scope
/// (a frame/scope operation), then describe the body as a [`Next`] — `Tail` of the non-tail
/// statements + the last, or `Suspend` for a deferred return. `ctx` is owned; the carrier
/// lifetime of `func` is free — only read.
///
/// TODO(rewrite): bind params via `ctx.arena.with_anchored_child`, split via
/// `super::body::split_body_statements`, return a `Next::Tail` of effects + last statement.
pub fn run_user_fn<'a>(
    _func: &KFunction<'_>,
    _bundle: ArgumentBundle<'a>,
    _ctx: Frame,
) -> Next<'a> {
    todo!("user-fn body entry — bind params, return a Tail of effects + last statement")
}
