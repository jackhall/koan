use std::fmt;
use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::scope_id::ScopeId;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    KKind, KType, NominalMember, NominalSchema, Parseable, Record, RecursiveSet,
};
use crate::machine::model::values::{KObject, NonWrappedRef};
use crate::machine::RegionBrand;
use crate::source::{self, FileId, SourceLoc, Span};

/// Structured runtime error propagated as a value via the `Err` arm of a node result. `frames` accumulate
/// as the error walks up the call graph; innermost call is `frames[0]`.
#[derive(Clone)]
pub struct KError {
    pub kind: KErrorKind,
    pub frames: Vec<TraceFrame>,
}

#[derive(Clone)]
pub enum KErrorKind {
    TypeMismatch {
        arg: String,
        expected: String,
        got: String,
    },
    MissingArg(String),
    UnboundName(String),
    ArityMismatch {
        expected: usize,
        got: usize,
    },
    /// Multiple registered functions matched with equal specificity.
    AmbiguousDispatch {
        expr: String,
        candidates: usize,
    },
    DispatchFailed {
        expr: String,
        reason: String,
    },
    /// A builtin's structural assumption about an argument's shape didn't hold.
    ShapeError(String),
    ParseError {
        message: String,
        span: Option<Span>,
        file: Option<FileId>,
    },
    /// In-language `RAISE`-style builtin landing pad.
    User(String),
    /// Same-scope rebind rejected; cross-scope shadowing remains allowed.
    Rebind {
        name: String,
    },
    /// Distinct from `Rebind` — collision is per-signature within the same name's bucket.
    DuplicateOverload {
        name: String,
        signature: String,
    },
    /// LET on a Type-class binder with a non-type RHS. `got` is the rendered
    /// name of the offending value's type (e.g. `"Number"`), pre-stringified
    /// so `KError` stays lifetime-free.
    TypeClassBindingExpectsType {
        name: String,
        got: String,
    },
    /// Scheduler drained its work queues with nodes still parked on
    /// dependencies that can no longer fire (dependency cycle).
    SchedulerDeadlock {
        pending: usize,
        sample: String,
    },
}

/// One entry in an error's call-stack trace. `function` and `expression` are
/// `summarize()` text; `location` is `Some` when the originating `KExpression`
/// had both `span` and `file` populated.
#[derive(Clone)]
pub struct TraceFrame {
    pub function: String,
    pub expression: String,
    pub location: Option<SourceLoc>,
}

impl TraceFrame {
    /// Locationless frame for call sites without an originating `KExpression`.
    pub fn bare(function: impl Into<String>, expression: impl Into<String>) -> TraceFrame {
        TraceFrame {
            function: function.into(),
            expression: expression.into(),
            location: None,
        }
    }

    pub fn for_call(function: &KFunction<'_>, expr: &KExpression<'_>) -> TraceFrame {
        TraceFrame {
            function: function.summarize(),
            expression: expr.summarize(),
            location: location_from_expr(expr),
        }
    }

    /// TraceFrame keyed off a `KExpression` but with a caller-chosen `function`
    /// label (e.g. `"<bind>"`) for scheduler-internal frames without a real
    /// `KFunction`.
    pub fn from_expr(function: impl Into<String>, expr: &KExpression<'_>) -> TraceFrame {
        TraceFrame {
            function: function.into(),
            expression: expr.summarize(),
            location: location_from_expr(expr),
        }
    }
}

fn location_from_expr(expr: &KExpression<'_>) -> Option<SourceLoc> {
    expr.span.zip(expr.file).map(|(span, file)| {
        source::with(file, |f| {
            let (line, col_utf16) = f.resolve(span.start);
            SourceLoc {
                path: f.path.clone(),
                line,
                col_utf16,
            }
        })
    })
}

impl KError {
    pub fn new(kind: KErrorKind) -> Self {
        Self {
            kind,
            frames: Vec::new(),
        }
    }

    /// Parse-pass error constructor. Resolves `file` from the thread-local
    /// `CURRENT_FILE` so call sites only thread the observed `Span`.
    pub fn parse(msg: impl Into<String>, span: Option<Span>) -> Self {
        Self::new(KErrorKind::ParseError {
            message: msg.into(),
            span,
            file: source::current(),
        })
    }

    pub fn with_frame(mut self, frame: TraceFrame) -> Self {
        self.frames.push(frame);
        self
    }

    pub fn with_call_frame(self, function: &KFunction<'_>, expr: &KExpression<'_>) -> Self {
        self.with_frame(TraceFrame::for_call(function, expr))
    }

    /// Spelled out (vs. `Clone`) so propagation sites read as intent.
    pub fn clone_for_propagation(&self) -> Self {
        self.clone()
    }

    /// Lower this error into a `KObject::Tagged` for `TRY-WITH` to dispatch
    /// on. The `tag` is the capitalized `KErrorKind` variant name (e.g. `"TypeMismatch"`),
    /// a valid type-token tag a TRY arm catches by name; the payload is a record-repr
    /// `KObject::Wrapped` mirroring the variant's fields plus `frames :List<Str>`, so TRY's
    /// `it.field` ATTR reads through the `Wrapped` arm. The payload's `type_id` and the
    /// wrapping `Tagged`'s `set` are synthetic singleton [`RecursiveSet`]s (named after the
    /// variant / `"KError"`, scope [`ScopeId::SENTINEL`]) because TRY's branch walker reads
    /// `tag` and `value` directly without going through dispatch — these carriers never need
    /// real nominal identity.
    ///
    /// `region` homes the payload's `&'a` `type_id`. It is the call-site scope's region, like
    /// any newtype's construction-site identity; unlike a declared NEWTYPE (whose identity
    /// lives in its outer declaring scope), this synthetic identity is minted here, so a TRY
    /// arm that returns the raw payload across a frame boundary inherits the general
    /// `Wrapped.type_id` re-anchor gap (the `inner` record itself rides an `Rc` and is
    /// lift-safe).
    pub fn to_tagged<'a>(&self, region: RegionBrand<'a>) -> KObject<'a> {
        let (name, fields) = self.kind.to_struct_fields();
        let frames_list = KObject::list(
            self.frames
                .iter()
                .map(|f| {
                    let base = format!("in {} ({})", f.expression, f.function);
                    let rendered = match &f.location {
                        Some(loc) => {
                            format!("{} at {}:{}:{}", base, loc.path, loc.line, loc.col_utf16)
                        }
                        None => base,
                    };
                    KObject::KString(rendered)
                })
                .collect(),
        );
        let mut pairs: Vec<(String, KObject<'a>)> = fields;
        pairs.push(("frames".to_string(), frames_list));
        let record = KObject::record(Record::from_pairs(pairs));
        // A freshly-minted synthetic `Rc` every call — no external identity to preserve, but a
        // `SetRef` never rebuilds at `'static` regardless, so it takes the checked path; its
        // members are always owned (`synthetic_singleton` never embeds a region pointer), so the
        // audit can never actually reject.
        let type_id: &'a KType<'a> = region
            .alloc_ktype_checked(KType::SetRef {
                set: synthetic_singleton(name.clone(), KKind::NewType),
                index: 0,
            })
            .expect("a freshly synthesized KError SetRef is always region-pure");
        let payload = KObject::Wrapped {
            inner: NonWrappedRef::peel(&record),
            type_id,
        };
        KObject::Tagged {
            tag: name,
            value: Rc::new(payload),
            set: synthetic_singleton("KError".to_string(), KKind::Tagged),
            index: 0,
            type_args: Rc::new(vec![]),
        }
    }
}

/// A throwaway singleton `RecursiveSet` for an unregistered carrier (the `KError`
/// to-tagged payload `type_id` and union `set`). Its one member carries an empty schema —
/// these carriers are read directly by the TRY branch walker, never dispatched on, so the
/// schema is never consulted.
fn synthetic_singleton<'a>(name: String, kind: KKind) -> Rc<RecursiveSet<'a>> {
    let member = NominalMember::pending(name, ScopeId::SENTINEL, kind);
    member.fill(match kind {
        KKind::NewType => NominalSchema::NewType(Box::new(KType::Any)),
        _ => NominalSchema::Tagged(std::collections::HashMap::new()),
    });
    Rc::new(RecursiveSet::new(vec![member]))
}

/// The `KError` carrier type — the `Tagged`-kind `SetRef` a `to_tagged` value reports its
/// family from. Used as the `Error` arm of `CATCH`'s declared `:(Result Any KError)` return
/// (a documentary contract — `KError` is not a registered prelude type, and the synthetic set
/// is identity-throwaway, but `CATCH`'s return is never validated against the runtime value).
pub(crate) fn kerror_ktype<'a>() -> KType<'a> {
    KType::SetRef {
        set: synthetic_singleton("KError".to_string(), KKind::Tagged),
        index: 0,
    }
}

impl KErrorKind {
    /// `(name, fields)` for `KError::to_tagged`. `name` is the capitalized variant tag —
    /// a TRY arm catches it by name (`TypeMismatch -> …`) — and also the payload newtype's
    /// identity. Field order mirrors the variant's declaration order; `frames` is appended
    /// by the caller. Dispatcher-internal kinds flatten to `{ kind, message }` since
    /// they're only catchable via `_`.
    fn to_struct_fields<'a>(&self) -> (String, Vec<(String, KObject<'a>)>) {
        match self {
            KErrorKind::TypeMismatch { arg, expected, got } => (
                "TypeMismatch".to_string(),
                vec![
                    ("arg".to_string(), KObject::KString(arg.clone())),
                    ("expected".to_string(), KObject::KString(expected.clone())),
                    ("got".to_string(), KObject::KString(got.clone())),
                ],
            ),
            KErrorKind::MissingArg(name) => (
                "MissingArg".to_string(),
                vec![("name".to_string(), KObject::KString(name.clone()))],
            ),
            KErrorKind::UnboundName(name) => (
                "UnboundName".to_string(),
                vec![("name".to_string(), KObject::KString(name.clone()))],
            ),
            KErrorKind::ArityMismatch { expected, got } => (
                "ArityMismatch".to_string(),
                vec![
                    ("expected".to_string(), KObject::Number(*expected as f64)),
                    ("got".to_string(), KObject::Number(*got as f64)),
                ],
            ),
            KErrorKind::AmbiguousDispatch { expr, candidates } => (
                "AmbiguousDispatch".to_string(),
                vec![
                    ("expr".to_string(), KObject::KString(expr.clone())),
                    (
                        "candidates".to_string(),
                        KObject::Number(*candidates as f64),
                    ),
                ],
            ),
            KErrorKind::DispatchFailed { expr, reason } => (
                "DispatchFailed".to_string(),
                vec![
                    ("expr".to_string(), KObject::KString(expr.clone())),
                    ("reason".to_string(), KObject::KString(reason.clone())),
                ],
            ),
            KErrorKind::ShapeError(msg) => (
                "ShapeError".to_string(),
                vec![("message".to_string(), KObject::KString(msg.clone()))],
            ),
            KErrorKind::ParseError {
                message,
                span,
                file,
            } => {
                let mut fields: Vec<(String, KObject<'a>)> = Vec::with_capacity(6);
                fields.push(("message".to_string(), KObject::KString(message.clone())));
                let (path, line, col_utf16) = match (span, file) {
                    (Some(sp), Some(fid)) => source::with(*fid, |f| {
                        let (line, col_utf16) = f.resolve(sp.start);
                        (Some(f.path.to_string()), Some(line), Some(col_utf16))
                    }),
                    _ => (None, None, None),
                };
                let (span_start, span_end) = match span {
                    Some(sp) => (Some(sp.start), Some(sp.end)),
                    None => (None, None),
                };
                // Raw offsets surface even when file lookup misses so
                // in-language consumers can pattern-match on byte ranges;
                // resolved fields fall back to "" / 0.
                fields.push((
                    "span_start".to_string(),
                    KObject::Number(span_start.unwrap_or(0) as f64),
                ));
                fields.push((
                    "span_end".to_string(),
                    KObject::Number(span_end.unwrap_or(0) as f64),
                ));
                fields.push((
                    "path".to_string(),
                    KObject::KString(path.unwrap_or_default()),
                ));
                fields.push((
                    "line".to_string(),
                    KObject::Number(line.unwrap_or(0) as f64),
                ));
                fields.push((
                    "col_utf16".to_string(),
                    KObject::Number(col_utf16.unwrap_or(0) as f64),
                ));
                ("ParseError".to_string(), fields)
            }
            KErrorKind::User(msg) => (
                "User".to_string(),
                vec![("message".to_string(), KObject::KString(msg.clone()))],
            ),
            KErrorKind::Rebind { .. }
            | KErrorKind::DuplicateOverload { .. }
            | KErrorKind::TypeClassBindingExpectsType { .. }
            | KErrorKind::SchedulerDeadlock { .. } => {
                let name = match self {
                    KErrorKind::Rebind { .. } => "Rebind",
                    KErrorKind::DuplicateOverload { .. } => "DuplicateOverload",
                    KErrorKind::TypeClassBindingExpectsType { .. } => "TypeClassBindingExpectsType",
                    KErrorKind::SchedulerDeadlock { .. } => "SchedulerDeadlock",
                    _ => unreachable!(),
                };
                (
                    name.to_string(),
                    vec![
                        ("kind".to_string(), KObject::KString(name.to_string())),
                        ("message".to_string(), KObject::KString(format!("{self}"))),
                    ],
                )
            }
        }
    }
}

impl fmt::Display for KError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        for frame in &self.frames {
            write!(f, "\n  in {} ({})", frame.expression, frame.function)?;
            if let Some(loc) = &frame.location {
                write!(f, " at {}:{}:{}", loc.path, loc.line, loc.col_utf16)?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for KErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KErrorKind::TypeMismatch { arg, expected, got } => {
                write!(
                    f,
                    "type mismatch for argument '{arg}': expected {expected}, got {got}"
                )
            }
            KErrorKind::MissingArg(name) => write!(f, "missing argument '{name}'"),
            KErrorKind::UnboundName(name) => write!(f, "unbound name '{name}'"),
            KErrorKind::ArityMismatch { expected, got } => {
                write!(
                    f,
                    "arity mismatch = expected {expected} arguments, got {got}"
                )
            }
            KErrorKind::AmbiguousDispatch { expr, candidates } => write!(
                f,
                "ambiguous dispatch: {candidates} candidates match {expr} with equal specificity",
            ),
            KErrorKind::DispatchFailed { expr, reason } => {
                write!(f, "dispatch failed for {expr}: {reason}")
            }
            KErrorKind::ShapeError(reason) => write!(f, "shape error: {reason}"),
            KErrorKind::ParseError {
                message,
                span,
                file,
            } => {
                let loc = match (span, file) {
                    (Some(sp), Some(fid)) => source::with(*fid, |sf| {
                        let (line, col_utf16) = sf.resolve(sp.start);
                        Some((sf.path.clone(), line, col_utf16))
                    }),
                    _ => None,
                };
                match loc {
                    Some((path, line, col)) => {
                        write!(f, "parse error at {path}:{line}:{col}: {message}")
                    }
                    None => write!(f, "parse error: {message}"),
                }
            }
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
                "type-class binding `{name}` expects a type value, got `{got}`",
            ),
            KErrorKind::SchedulerDeadlock { pending, sample } => write!(
                f,
                "scheduler deadlock: {pending} node(s) left unresolved on a dependency \
                 cycle (e.g. `{sample}`)",
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
mod tests;
