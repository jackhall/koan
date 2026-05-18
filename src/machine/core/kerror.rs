use std::fmt;

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::types::Parseable;
use crate::machine::model::KType;
use crate::machine::model::ast::KExpression;

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
    /// LET on a Type-class binder with a non-type RHS — caught at bind time
    /// rather than at downstream elaboration. Pairs with stage 1.7's routing flip.
    TypeClassBindingExpectsType { name: String, got: KType },
    /// A `TypeNameRef` carrier landed at the dispatch boundary's per-call
    /// parameter dual-write (`type_identity_for`) but its `TypeExpr` couldn't
    /// be elaborated in the FN's captured definition scope because some
    /// referenced type-binding is still pending finalization. Replaces today's
    /// silent skip — surfaces the precise context (parameter, surface form,
    /// pending finalize-node) so a workload that hits this regularly is
    /// debuggable without diving into the dispatcher's internals.
    TypeIdentityPendingAtDispatch {
        param: String,
        surface: String,
        pending_on: Vec<crate::machine::core::kfunction::NodeId>,
    },
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
                write!(f, "arity mismatch = expected {expected} arguments, got {got}")
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
            KErrorKind::TypeClassBindingExpectsType { name, got } => write!(
                f,
                "type-class binding `{name}` expects a type value, got `{}`",
                got.name(),
            ),
            KErrorKind::TypeIdentityPendingAtDispatch { param, surface, pending_on } => write!(
                f,
                "per-call type identity for `{param}` (surface form `{surface}`) is \
                 pending finalize on producer node(s) {pending_on:?}",
            ),
        }
    }
}

impl fmt::Debug for KError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

#[cfg(test)]
mod tests {
    //! `Display`-rendering round-trip per `KErrorKind` variant. Pins format strings against
    //! accidental rewording — if you change a message, update the matching test here.
    use super::*;
    use crate::machine::core::kfunction::NodeId;

    fn render(kind: KErrorKind) -> String { format!("{}", KError::new(kind)) }

    #[test]
    fn display_type_mismatch() {
        let s = render(KErrorKind::TypeMismatch {
            arg: "x".into(),
            expected: "Number".into(),
            got: "Str".into(),
        });
        assert_eq!(s, "type mismatch for argument 'x': expected Number, got Str");
    }

    #[test]
    fn display_missing_arg() {
        assert_eq!(render(KErrorKind::MissingArg("y".into())), "missing argument 'y'");
    }

    #[test]
    fn display_unbound_name() {
        assert_eq!(render(KErrorKind::UnboundName("foo".into())), "unbound name 'foo'");
    }

    #[test]
    fn display_arity_mismatch() {
        let s = render(KErrorKind::ArityMismatch { expected: 2, got: 3 });
        assert_eq!(s, "arity mismatch = expected 2 arguments, got 3");
    }

    #[test]
    fn display_ambiguous_dispatch() {
        let s = render(KErrorKind::AmbiguousDispatch {
            expr: "(F 1)".into(),
            candidates: 2,
        });
        assert_eq!(s, "ambiguous dispatch: 2 candidates match (F 1) with equal specificity");
    }

    #[test]
    fn display_dispatch_failed() {
        let s = render(KErrorKind::DispatchFailed {
            expr: "(G 1)".into(),
            reason: "no overload accepts Number".into(),
        });
        assert_eq!(s, "dispatch failed for (G 1): no overload accepts Number");
    }

    #[test]
    fn display_shape_error() {
        assert_eq!(render(KErrorKind::ShapeError("bad parts".into())), "shape error: bad parts");
    }

    #[test]
    fn display_parse_error() {
        assert_eq!(render(KErrorKind::ParseError("eof".into())), "parse error: eof");
    }

    #[test]
    fn display_user_message_is_verbatim() {
        assert_eq!(render(KErrorKind::User("boom".into())), "boom");
    }

    #[test]
    fn display_rebind() {
        let s = render(KErrorKind::Rebind { name: "x".into() });
        assert_eq!(s, "name 'x' is already bound in this scope");
    }

    #[test]
    fn display_duplicate_overload() {
        let s = render(KErrorKind::DuplicateOverload {
            name: "F".into(),
            signature: "(Number)".into(),
        });
        assert_eq!(s, "function 'F' already has an overload with signature (Number)");
    }

    #[test]
    fn display_type_class_binding_expects_type() {
        let s = render(KErrorKind::TypeClassBindingExpectsType {
            name: "T".into(),
            got: KType::Number,
        });
        assert_eq!(s, "type-class binding `T` expects a type value, got `Number`");
    }

    #[test]
    fn display_type_identity_pending_at_dispatch() {
        let s = render(KErrorKind::TypeIdentityPendingAtDispatch {
            param: "x".into(),
            surface: "List<T>".into(),
            pending_on: vec![NodeId(7)],
        });
        assert_eq!(
            s,
            "per-call type identity for `x` (surface form `List<T>`) is \
             pending finalize on producer node(s) [NodeId(7)]",
        );
    }

    #[test]
    fn with_frame_renders_call_stack_inline() {
        let err = KError::new(KErrorKind::User("boom".into()))
            .with_frame(Frame { function: "F".into(), expression: "(F 1)".into() })
            .with_frame(Frame { function: "G".into(), expression: "(G (F 1))".into() });
        assert_eq!(err.to_string(), "boom\n  in (F 1) (F)\n  in (G (F 1)) (G)");
    }

    #[test]
    fn debug_matches_display() {
        let err = KError::new(KErrorKind::MissingArg("z".into()))
            .with_frame(Frame { function: "F".into(), expression: "(F)".into() });
        assert_eq!(format!("{:?}", err), format!("{}", err));
    }

    #[test]
    fn clone_for_propagation_preserves_kind_and_frames() {
        let err = KError::new(KErrorKind::UnboundName("q".into()))
            .with_frame(Frame { function: "H".into(), expression: "(H q)".into() });
        let copy = err.clone_for_propagation();
        assert_eq!(copy.to_string(), err.to_string());
        assert_eq!(copy.frames.len(), 1);
    }
}
