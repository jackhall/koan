use std::fmt;

use crate::dispatch::kfunction::KFunction;
use crate::dispatch::types::Parseable;
use crate::parse::KExpression;

/// Structured runtime error propagated as a value via `BodyResult::Err`. `frames` accumulate
/// as the error walks up the call graph; innermost call is `frames[0]`.
#[derive(Clone)]
pub struct KError {
    pub kind: KErrorKind,
    pub frames: Vec<Frame>,
}

#[derive(Clone)]
pub enum KErrorKind {
    TypeMismatch { arg: String, expected: String, got: String },
    MissingArg(String),
    UnboundName(String),
    ArityMismatch { expected: usize, got: usize },
    /// Multiple registered functions matched with equal specificity.
    AmbiguousDispatch { expr: String, candidates: usize },
    DispatchFailed { expr: String, reason: String },
    /// A builtin's structural assumption about an argument's shape didn't hold.
    ShapeError(String),
    ParseError(String),
    /// In-language `RAISE`-style builtin landing pad.
    User(String),
    /// Same-scope rebind rejected; cross-scope shadowing remains allowed.
    Rebind { name: String },
    /// Distinct from `Rebind` — collision is per-signature within the same name's bucket.
    DuplicateOverload { name: String, signature: String },
}

/// One entry in an error's call-stack trace. Both fields are `summarize()` text because
/// `KExpression` doesn't carry source spans yet.
#[derive(Clone)]
pub struct Frame {
    pub function: String,
    pub expression: String,
}

impl Frame {
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

    pub fn with_frame(mut self, frame: Frame) -> Self {
        self.frames.push(frame);
        self
    }

    pub fn with_call_frame(self, function: &KFunction<'_>, expr: &KExpression<'_>) -> Self {
        self.with_frame(Frame::for_call(function, expr))
    }

    /// Spelled out (vs. `Clone`) so propagation sites read as intent rather than mechanism.
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
