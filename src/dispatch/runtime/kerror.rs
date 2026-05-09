use std::fmt;

use crate::dispatch::kfunction::KFunction;
use crate::dispatch::types::Parseable;
use crate::parse::kexpression::KExpression;

/// A structured runtime error. Replaces the prior pattern of returning `KObject::Null` from
/// every failure path in builtins and stringly-typed `Result<_, String>` from the scheduler.
/// Errors propagate as values via `BodyResult::Err`; the scheduler short-circuits any node
/// whose dependency errored, appending a `Frame` as the error walks up the call graph.
///
/// `kind` carries the structured failure reason; `frames` accumulate context as the error
/// passes through user-fn invocations and Bind/Dispatch nodes. Innermost call is `frames[0]`.
#[derive(Clone)]
pub struct KError {
    pub kind: KErrorKind,
    pub frames: Vec<Frame>,
}

/// What went wrong. Each variant captures enough structured detail that the CLI's display can
/// name the offending argument/expression without reverse-engineering it from a string.
#[derive(Clone)]
pub enum KErrorKind {
    /// An argument resolved to a value of the wrong KObject variant.
    TypeMismatch { arg: String, expected: String, got: String },
    /// An argument expected by the signature wasn't present in the bundle.
    MissingArg(String),
    /// A name lookup found no binding in the scope chain.
    UnboundName(String),
    /// `KFunction::apply` was handed too few or too many positional args for the function's
    /// signature.
    ArityMismatch { expected: usize, got: usize },
    /// Multiple registered functions matched an expression with equal specificity.
    AmbiguousDispatch { expr: String, candidates: usize },
    /// No registered function matched the expression's shape.
    DispatchFailed { expr: String, reason: String },
    /// A builtin's structural assumption about an argument's shape (typically the
    /// `Rc::try_unwrap` + variant-match dance for `KType::KExpression` slots) didn't hold.
    ShapeError(String),
    /// Wraps the parser's stringly-typed errors so they flow through the same channel as
    /// runtime errors. Future cleanup may break this into structured parse-error variants.
    ParseError(String),
    /// Landing pad for an in-language `RAISE`-style builtin, not yet shipped.
    User(String),
    /// A binder (LET, STRUCT, UNION, SIG, MODULE) tried to install a value name that the
    /// current scope already binds — same-scope rebind is rejected per the decided rule
    /// (cross-scope shadowing remains allowed). Surfaces via `Scope::bind_value` and the
    /// dispatch-time placeholder install path (`install_dispatch_placeholder`).
    Rebind { name: String },
    /// A FN registration found an existing overload in the same bucket whose
    /// `ExpressionSignature` is exact-equal (same shape, same per-slot KType). Distinct from
    /// `Rebind` — function names live in a separate `functions` table keyed by signature, so
    /// the collision is per-signature rather than per-name.
    DuplicateOverload { name: String, signature: String },
}

/// One entry in an error's call-stack trace. `function` is the registered function's
/// `summarize()` (e.g. `fn(MATCH <value> WITH <branches>)`); `expression` is the expression
/// being evaluated when the error surfaced (`KExpression::summarize`). Source spans aren't
/// available — `KExpression` doesn't carry them yet — so both fields are textual summaries.
/// Adding spans later is non-breaking because `Frame` is a struct.
#[derive(Clone)]
pub struct Frame {
    pub function: String,
    pub expression: String,
}

impl Frame {
    /// Build a frame from a `(function, expression)` pair — the common shape the scheduler
    /// constructs when propagating an error past a user-fn invocation. Captures the two
    /// `summarize()` calls in one place so the propagation site reads as a one-liner. The
    /// function's summary populates both the `function` field (signature-text identifier)
    /// and the `expression` field (the user wrote the verb at the call site, so the
    /// expression-text of the Bind is the same surface text as the function's signature).
    pub fn for_call(function: &KFunction<'_>, expr: &KExpression<'_>) -> Frame {
        Frame {
            function: function.summarize(),
            expression: expr.summarize(),
        }
    }
}

impl KError {
    pub fn new(kind: KErrorKind) -> Self {
        Self { kind, frames: Vec::new() }
    }

    /// Append a frame to the error's call stack and return self by value. Used by the
    /// scheduler when an errored sub-node propagates up through a Bind: the parent's
    /// expression text becomes the next frame so the trace reconstructs the call chain.
    pub fn with_frame(mut self, frame: Frame) -> Self {
        self.frames.push(frame);
        self
    }

    /// Convenience: build a `Frame::for_call` and append in one call. The common scheduler
    /// propagation shape is "an inner call errored — wrap with the function's signature
    /// text"; this wrapper collapses the two-step `with_frame(Frame::for_call(...))` to a
    /// single method call.
    pub fn with_call_frame(self, function: &KFunction<'_>, expr: &KExpression<'_>) -> Self {
        self.with_frame(Frame::for_call(function, expr))
    }

    /// Owned copy for the propagation path: when the scheduler's `read_result` hands back
    /// a `&KError` referencing a sub-node's stored result, the parent needs an owned
    /// `KError` to install in its own slot (and to append a frame to). Same shape as
    /// `Clone` but spelled out so the call site reads as "I'm taking this for propagation."
    pub fn clone_for_propagation(&self) -> Self {
        self.clone()
    }
}

impl fmt::Display for KError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        for frame in &self.frames {
            write!(f, "\n  in {} ({})", frame.expression, frame.function)?;
        }
        Ok(())
    }
}

impl fmt::Display for KErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                write!(f, "type mismatch for argument '{arg}': expected {expected}, got {got}")
            }
            KErrorKind::MissingArg(name) => write!(f, "missing argument '{name}'"),
            KErrorKind::UnboundName(name) => write!(f, "unbound name '{name}'"),
            KErrorKind::ArityMismatch { expected, got } => {
                write!(f, "arity mismatch: expected {expected} arguments, got {got}")
            }
            KErrorKind::AmbiguousDispatch { expr, candidates } => write!(
                f,
                "ambiguous dispatch: {candidates} candidates match {expr} with equal specificity",
            ),
            KErrorKind::DispatchFailed { expr, reason } => {
                write!(f, "dispatch failed for {expr}: {reason}")
            }
            KErrorKind::ShapeError(reason) => write!(f, "shape error: {reason}"),
            KErrorKind::ParseError(reason) => write!(f, "parse error: {reason}"),
            KErrorKind::User(msg) => write!(f, "{msg}"),
            KErrorKind::Rebind { name } => {
                write!(f, "name '{name}' is already bound in this scope")
            }
            KErrorKind::DuplicateOverload { name, signature } => write!(
                f,
                "function '{name}' already has an overload with signature {signature}",
            ),
        }
    }
}

impl fmt::Debug for KError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}
