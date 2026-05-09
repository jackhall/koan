//! `KFunction` and the scheduler-facing types it depends on. A `KFunction` carries an
//! `ExpressionSignature` (its call shape — defined in [`super::types::signature`]), a
//! `Body` (builtin `fn` pointer or captured user-defined `KExpression`), and a captured
//! scope for lexical lookup of free names. The `bind` / `apply` methods produce a `KFuture`
//! (positional) or a tail-rewriting `BodyResult` (named-argument) that the scheduler runs.
//!
//! Sits at the dispatch root because it integrates all three layers: [`super::types`] for
//! `KType` / `ExpressionSignature` / `Argument`, [`super::values`] for the `KObject`s it
//! produces and consumes, and [`super::runtime`] for the arena / scope / error plumbing.

use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::runtime::{CallArena, KError, KErrorKind, KFuture, Scope};
use super::types::{ExpressionSignature, Parseable, SignatureElement};
use super::values::KObject;

/// Stable handle to a node in the scheduler's DAG. Lives here (rather than `execute/scheduler`)
/// so `BodyResult::Defer` can name a node without `dispatch` having to import from `execute` —
/// see the module-level note on `SchedulerHandle` for the layering rationale.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize { self.0 }
}

/// Side-channel a builtin body uses to spawn additional `Dispatch` nodes during the scheduler's
/// run. Defined in `dispatch` (rather than as inherent methods on `Scheduler`) so `BuiltinFn`
/// can reference it without dragging the whole scheduler module into `dispatch`'s import graph.
/// `Scheduler` impls this trait in `execute/scheduler.rs`. Two methods: `add_dispatch` is the
/// classic lever ("schedule this expression for late dispatch in the given scope, give me a
/// NodeId"). `current_frame` returns the active slot's `Rc<CallArena>` (if any) so a builtin
/// that builds its own per-call frame whose child scope's `outer` points into the call site
/// can chain that Rc onto the new frame — keeping the call-site arena alive while the new
/// frame is in use. Without this, MATCH-style builtins (whose new frame's outer is a per-call
/// scope, not a captured lexical scope) hand out a frame whose `outer` becomes dangling the
/// moment the slot's old frame is dropped on TCO replace.
pub trait SchedulerHandle<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId;
    fn current_frame(&self) -> Option<Rc<CallArena>>;
}

/// What a builtin's body returns. `Value` is the common case — the body computed its result
/// inline. `Tail { expr, frame }` says "my result is whatever this expression produces,
/// evaluate it in place"; the scheduler rewrites the current node's work to a fresh
/// `Dispatch(expr)` and re-runs the same slot. `frame = Some(f)` installs the per-call
/// `CallArena` `f` in the slot — its scope becomes the slot's scope and its arena owns the
/// per-call allocations. Used by `KFunction::invoke` for user-defined bodies. `frame = None`
/// keeps the slot's existing frame and scope (used by builtins whose tail expression
/// evaluates in the same frame as the call site).
/// A chain of tail calls reuses one slot rather than allocating a new one per step; a TCO
/// replace with `frame = Some` drops the slot's previous frame immediately. For user-fn
/// invokes that's safe by lexical scoping (the new frame's child scope's `outer` is the
/// FN's captured scope, not the previous frame's). For builtins that build a frame whose
/// child scope's `outer` IS the call site (MATCH), the new frame holds the previous
/// frame's Rc via `CallArena::outer_frame` so dropping the slot's prev_frame here doesn't
/// free memory the new frame still references. `Err(KError)` propagates a structured
/// failure; the scheduler short-circuits any node whose dependency errored, appending a
/// `Frame` as the error walks up.
pub enum BodyResult<'a> {
    Value(&'a KObject<'a>),
    Tail {
        expr: KExpression<'a>,
        frame: Option<Rc<CallArena>>,
        /// User-fn reference attached to the slot for two purposes: (1) the slot's Done arm
        /// reads `signature.return_type` to enforce the declared return type at runtime, and
        /// (2) on error, `function.summarize()` becomes the appended `Frame`'s function name
        /// so the call-stack trace identifies which user-fn the error happened inside.
        /// `Some(f)` for `KFunction::invoke`'s UserDefined path; `None` for builtin tails
        /// that are deferred-eval continuations, not calls.
        function: Option<&'a KFunction<'a>>,
    },
    Err(KError),
}

impl<'a> BodyResult<'a> {
    /// Tail return that keeps the slot's existing frame and scope. Used by builtins whose
    /// tail expression evaluates in the same frame as the call site.
    pub fn tail(expr: KExpression<'a>) -> Self {
        BodyResult::Tail { expr, frame: None, function: None }
    }

    /// Tail return that installs a fresh per-call frame on the slot. Used by
    /// `KFunction::invoke` for user-defined bodies — `frame` is an `Rc` to the per-call
    /// arena and the child scope holding bound parameters. Other Rcs (e.g., escaping
    /// closures, future stages) may share ownership. `function` is the called user-fn,
    /// kept on the slot for return-type enforcement and error-frame attribution.
    pub fn tail_with_frame(
        expr: KExpression<'a>,
        frame: Rc<CallArena>,
        function: &'a KFunction<'a>,
    ) -> Self {
        BodyResult::Tail { expr, frame: Some(frame), function: Some(function) }
    }

    /// Error return. Wraps a `KError` so the scheduler can short-circuit dependents.
    pub fn err(e: KError) -> Self {
        BodyResult::Err(e)
    }
}

/// A function pointer that implements a builtin `KFunction`'s body. `for<'a>` so a single `fn`
/// works for any caller scope lifetime; the `&mut dyn SchedulerHandle<'a>` is the lever a body
/// uses to defer sub-expression evaluation back to the scheduler. `Scope` is shared (`&'a`)
/// rather than `&mut` because a single scope reference is used by every node spawned during a
/// per-call body's evaluation; mutability is interior (RefCell).
pub type BuiltinFn = for<'a> fn(
    &'a Scope<'a>,
    &mut dyn SchedulerHandle<'a>,
    ArgumentBundle<'a>,
) -> BodyResult<'a>;

/// Dispatch-time name extractor for a binder builtin. Invoked by `run_dispatch` *before* any
/// sub-deps are scheduled: given the unresolved expression that's about to dispatch, return
/// `Some(name)` to install a `placeholders[name] = NodeId(this_slot)` mapping in the
/// dispatching scope, or `None` to opt out (non-binder builtins, or a binder shape whose
/// name slot is missing/malformed and the body will surface a `ShapeError` later).
///
/// Each binder's extractor reads the structural shape of the unresolved expression — typically
/// `parts[1]` — without dispatching anything, so the install fires before the sub-expression
/// graph wakes. The placeholder lets a sibling expression looking up `name` while this slot's
/// body is still in flight park on this slot via the scheduler's `notify_list` /
/// `pending_deps` machinery (see [`crate::dispatch::runtime::Scope::resolve`]).
pub type PreRunFn = for<'a> fn(&KExpression<'a>) -> Option<String>;

/// What a `KFunction`'s body actually is. Builtins carry a host `fn` pointer; user-defined
/// functions carry a captured `KExpression` to be dispatched at call time. Kept as an enum
/// rather than a `Box<dyn Fn>` so the user-defined case stays introspectable — the upcoming TCO
/// and error-frame work both need to walk into the captured expression.
pub enum Body<'a> {
    Builtin(BuiltinFn),
    UserDefined(KExpression<'a>),
}

/// A callable Koan function: its `ExpressionSignature` (the call shape it matches), the body
/// implementation, and a captured scope. `Scope::dispatch` finds the right `KFunction` by
/// signature and then `bind`s a `KExpression` into a `KFuture`; the body runs via
/// `KFunction::invoke` at execute time.
///
/// `captured` is the lexical environment captured at definition time: for user-defined FNs
/// it's the scope that ran the `FN ...` form; for builtins it's the run-root scope (where
/// they were registered). User-fn bodies resolve free names through this chain — lexical
/// scoping. The captured pointer is lifetime-erased to `*const Scope<'static>` to keep
/// `KFunction<'a>` covariant in `'a`; storing a real `&'a Scope<'a>` would make `KFunction`
/// invariant (because `Scope<'a>` is invariant via its `RefCell`s) and would break builtin
/// registration's coercion from `'static` to shorter lifetimes. SAFETY: the captured scope
/// is allocated in a `RuntimeArena` that outlives this `KFunction` — they share the arena
/// (FN registers the function in the same scope it captures; builtins are registered in
/// run-root). See the `arena.rs` module-level note for the broader lifetime-erasure pattern.
pub struct KFunction<'a> {
    pub signature: ExpressionSignature,
    pub body: Body<'a>,
    captured: *const Scope<'static>,
    /// Dispatch-time placeholder extractor. `Some(_)` for the binder builtins (LET, FN,
    /// STRUCT, UNION, SIG, MODULE) — `run_dispatch` calls it before scheduling sub-deps and
    /// installs the returned name in the dispatching scope's `placeholders` so a forward
    /// reference parks on this slot until the binder's body finalizes. `None` for everything
    /// else (the dispatch-time short-circuit is a no-op for non-binders).
    pub pre_run: Option<PreRunFn>,
}

impl<'a> KFunction<'a> {
    /// Construct a `KFunction`. `captured` is the FN's defining scope (or, for builtins,
    /// run-root — the scope they're being registered into).
    pub fn new(
        signature: ExpressionSignature,
        body: Body<'a>,
        captured: &'a Scope<'a>,
    ) -> Self {
        Self::with_pre_run(signature, body, captured, None)
    }

    /// `KFunction::new` with an optional dispatch-time placeholder extractor. The binder
    /// builtins (LET, FN, STRUCT, UNION, SIG, MODULE) supply a `pre_run` so `run_dispatch`
    /// can install a `name → producer-NodeId` placeholder in the dispatching scope before
    /// scheduling sub-deps. See [`PreRunFn`] for the contract.
    pub fn with_pre_run(
        mut signature: ExpressionSignature,
        body: Body<'a>,
        captured: &'a Scope<'a>,
        pre_run: Option<PreRunFn>,
    ) -> Self {
        signature.normalize();
        // The double cast erases the lifetime to match the `*const Scope<'static>` field
        // type — `Scope` is invariant in `'a`, so a single cast is rejected.
        #[allow(clippy::unnecessary_cast)]
        let captured = captured as *const Scope<'_> as *const Scope<'static>;
        Self { signature, body, captured, pre_run }
    }

    /// Re-attach the captured scope pointer to a fresh `'a` lifetime. The lifetime tracks
    /// the original scope's allocation, which by the SAFETY argument on the struct still
    /// lives.
    pub fn captured_scope(&self) -> &'a Scope<'a> {
        unsafe { &*self.captured.cast::<Scope<'a>>() }
    }

    pub fn summarize(&self) -> String {
        let parts: Vec<String> = self
            .signature
            .elements
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(s) => s.clone(),
                SignatureElement::Argument(arg) => format!("<{}>", arg.name),
            })
            .collect();
        format!("fn({})", parts.join(" "))
    }

    pub fn bind(&'a self, expr: KExpression<'a>) -> Result<KFuture<'a>, KError> {
        if self.signature.elements.len() != expr.parts.len() {
            return Err(KError::new(KErrorKind::ArityMismatch {
                expected: self.signature.elements.len(),
                got: expr.parts.len(),
            }));
        }
        let mut args: HashMap<String, Rc<KObject<'a>>> = HashMap::new();
        for (el, part) in self.signature.elements.iter().zip(expr.parts.iter()) {
            match el {
                SignatureElement::Keyword(s) => match part {
                    ExpressionPart::Keyword(t) if s == t => {}
                    ExpressionPart::Keyword(t) => {
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: expr.summarize(),
                            reason: format!("expected keyword '{s}', got '{t}'"),
                        }));
                    }
                    _ => {
                        return Err(KError::new(KErrorKind::DispatchFailed {
                            expr: expr.summarize(),
                            reason: format!("expected keyword '{s}'"),
                        }));
                    }
                },
                SignatureElement::Argument(arg) => {
                    if !arg.matches(part) {
                        return Err(KError::new(KErrorKind::TypeMismatch {
                            arg: arg.name.clone(),
                            expected: arg.ktype.name(),
                            got: part.summarize(),
                        }));
                    }
                    args.insert(arg.name.clone(), Rc::new(part.resolve_for(&arg.ktype)));
                }
            }
        }
        Ok(KFuture {
            parsed: expr,
            function: self,
            bundle: ArgumentBundle { args },
        })
    }

    /// Apply this function to a **named** argument list, weaving the signature's keyword
    /// tokens back in. The caller passes the inner parts of `f (a: 1, b: 2)` and this method
    /// parses them as `<name>: <value>` triples (via
    /// [`parse_named_value_pairs`](super::named_pairs::parse_named_value_pairs)), validates
    /// names against the signature's `Argument` slot names, and reorders the values into
    /// signature order before emitting the tail.
    ///
    /// Validation precedence (when both fire, the first wins): missing arg → unknown arg →
    /// arity. Missing-first because telling the user "you forgot `b`" is more actionable
    /// than "you have a stray `c`".
    ///
    /// Returns `BodyResult::Tail` whose expression matches this function's keyword-bucketed
    /// signature on re-dispatch (positional values reordered by name). Errors map to
    /// `ShapeError` (malformed pair shape), `MissingArg`, or `ArityMismatch` as appropriate.
    ///
    /// Used by the [`call_by_name`](super::builtins::call_by_name) builtin's body to wire
    /// `f (a: 1)` to the underlying function's call. Lives on `KFunction` so the builtin's
    /// body stays a thin shim and the synthesis logic is co-located with the rest of "how
    /// to call a function."
    pub fn apply<'b>(&self, args: Vec<ExpressionPart<'b>>) -> BodyResult<'b> {
        let tmp_expr = KExpression { parts: args };
        let pairs = match super::values::parse_named_value_pairs(&tmp_expr, "function call") {
            Ok(p) => p,
            Err(msg) => return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg))),
        };
        let arg_names: Vec<&str> = self
            .signature
            .elements
            .iter()
            .filter_map(|el| match el {
                SignatureElement::Argument(a) => Some(a.name.as_str()),
                _ => None,
            })
            .collect();
        // Missing-first error precedence: any missing arg shadows arity / unknown checks.
        for name in &arg_names {
            if !pairs.iter().any(|(n, _)| n == name) {
                return BodyResult::Err(KError::new(KErrorKind::MissingArg((*name).to_string())));
            }
        }
        for (pair_name, _) in &pairs {
            if !arg_names.iter().any(|n| n == pair_name) {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "unknown name `{}` in function call",
                    pair_name
                ))));
            }
        }
        if pairs.len() != arg_names.len() {
            return BodyResult::Err(KError::new(KErrorKind::ArityMismatch {
                expected: arg_names.len(),
                got: pairs.len(),
            }));
        }
        let mut parts = Vec::with_capacity(self.signature.elements.len());
        for el in &self.signature.elements {
            match el {
                SignatureElement::Keyword(s) => parts.push(ExpressionPart::Keyword(s.clone())),
                SignatureElement::Argument(a) => {
                    let value_part = pairs
                        .iter()
                        .find(|(n, _)| n == &a.name)
                        .map(|(_, v)| v.clone())
                        .expect("missing-arg check above guarantees presence");
                    parts.push(value_part);
                }
            }
        }
        BodyResult::tail(KExpression { parts })
    }
}

/// Name → resolved value map produced by `KFunction::bind`; the concrete arguments a
/// `KFuture` will hand to its function body when executed.
pub struct ArgumentBundle<'a> {
    pub args: HashMap<String, Rc<KObject<'a>>>,
}

impl<'a> ArgumentBundle<'a> {
    pub fn get(&self, name: &str) -> Option<&KObject<'a>> {
        self.args.get(name).map(|v| v.as_ref())
    }

    /// Independent clone: each value is `deep_clone`d into a fresh `Rc`. The original bundle's
    /// `Rc`-shared values are not preserved as shared in the clone — `deep_clone`'s contract is
    /// "fully independent copy."
    pub fn deep_clone(&self) -> ArgumentBundle<'a> {
        ArgumentBundle {
            args: self
                .args
                .iter()
                .map(|(k, v)| (k.clone(), Rc::new(v.deep_clone())))
                .collect(),
        }
    }
}
