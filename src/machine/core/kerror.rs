use std::fmt;

use crate::machine::model::WorkingExpression;
use crate::machine::model::{Carried, CarriedFamily, KObject, Symbol};
use crate::machine::model::{
    KKind, KType, RecursiveGroupWindow, RelativeSchema, TypeMemberMap, TypeRegistry, TypeSymbol,
};
use crate::source::{self, FileId, SourceLoc, Span};
use crate::witnessed::RegionHandleFamily;

use super::{DeliveredCarried, FoldingBrand, RegionBrand, SubstrateDoor};
use super::{KoanStorageProfile, Scope, scope_frame};
use crate::machine::model::RunRegistries;

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
    /// A binder-introducing form (LET, FN, OP, TYPE, …) appeared in an eagerly evaluated
    /// sub-expression — a user-call or builtin argument, an operator operand, a literal element, a
    /// deferred head, or another binder's declaration slot — where a parse-time install cannot be
    /// sound. Binding is a statement-level act. Slot-terminal and TRY-catchable, like
    /// [`DispatchFailed`](KErrorKind::DispatchFailed). `expr` is the offending sub-expression's
    /// rendered form; `suggest_flat` is Display-only (the TRY-record projection exposes `expr`
    /// alone) and marks a rejected `FN` / `OP` definition, which has a one-statement spelling.
    NestedBinder {
        expr: String,
        suggest_flat: bool,
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
    /// Two statements of one block declare the same name. Distinct from `Rebind` because it is
    /// ruled on where the block's claim store is built — with both declaring statements in hand,
    /// before either runs — so it names both positions rather than landing on whichever body
    /// happened to commit second. Positions are lexical statement numbers, counting from 1.
    DuplicateDeclaration {
        name: String,
        first: usize,
        second: usize,
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
/// `summarize()` text; `location` is `Some` when the originating expression
/// had both `span` and `file` populated.
#[derive(Clone)]
pub struct TraceFrame {
    pub function: String,
    pub expression: String,
    pub location: Option<SourceLoc>,
}

impl TraceFrame {
    /// Locationless frame for call sites without an originating expression.
    pub fn bare(function: impl Into<String>, expression: impl Into<String>) -> TraceFrame {
        TraceFrame {
            function: function.into(),
            expression: expression.into(),
            location: None,
        }
    }

    /// TraceFrame keyed off the expression a slot is dispatching, with a caller-chosen `function`
    /// label (e.g. `"<bind>"`) for scheduler-internal frames without a real `KFunction`.
    pub fn from_expr(function: impl Into<String>, expr: &WorkingExpression<'_>) -> TraceFrame {
        TraceFrame {
            function: function.into(),
            expression: expr.summarize(),
            location: location_from_expr(expr),
        }
    }
}

fn location_from_expr(expr: &WorkingExpression<'_>) -> Option<SourceLoc> {
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

    /// Spelled out (vs. `Clone`) so propagation sites read as intent.
    pub fn clone_for_propagation(&self) -> Self {
        self.clone()
    }

    /// Lower this error into a `KObject::Tagged` for `TRY-WITH` to dispatch
    /// on. The `tag` is the capitalized `KErrorKind` variant name (e.g. `"TypeMismatch"`),
    /// a valid type-token tag a TRY arm catches by name; the payload is a record-repr
    /// `KObject::Wrapped` mirroring the variant's fields plus `frames :List<Str>`, so TRY's
    /// `it.field` ATTR reads through the `Wrapped` arm. The payload's `type_id` and the
    /// wrapping `Tagged`'s `identity` are synthetic singleton members (named after the variant /
    /// `"KError"`) because TRY's branch walker reads `tag` and `value` directly without going
    /// through dispatch — these carriers never need real nominal identity. They intern like any
    /// other member, so two errors of one variant carry one handle.
    ///
    /// `door` is the substrate door the payload's `Record` substrate is born through — a caller with
    /// no fold in hand mints a zero-dep one (see [`Self::to_tagged_delivered`]). Every cell here is
    /// freshly built owned data, so the door needs no holder.
    pub fn to_tagged<'a>(
        &self,
        door: SubstrateDoor<'a, '_>,
        registries: &RunRegistries,
    ) -> KObject<'a> {
        let types = &registries.types;
        let brand = **door;
        let (name, fields) = self.kind.to_struct_fields(brand);
        let frames_list = KObject::list(
            door,
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
                    KObject::KString(brand.allocator().text(&rendered))
                })
                .collect(),
            types,
        );
        // Every label here is a fixed literal of the error shape — syntactic in the same sense a
        // source-written field name is, so each interns and renders back through the interner.
        let mut pairs: Vec<(String, KObject<'a>)> = fields;
        pairs.push(("frames".to_string(), frames_list));
        let interned: Vec<(Symbol, KObject<'a>)> = pairs
            .into_iter()
            .map(|(name, value)| (registries.labels.intern(&name), value))
            .collect();
        let record = KObject::record(door, &interned, types);
        // The variant name and `KError` are fixed literals of the error shape, Type tokens by
        // construction; they intern here so a rendered member name resolves.
        let variant = error_label(&name, registries);
        let payload = KObject::wrapped_peel(
            door,
            &record,
            synthetic_singleton(variant, KKind::NewType, types),
        );
        KObject::tagged(
            door,
            &name,
            &payload,
            synthetic_singleton(
                error_label("KError", registries),
                KKind::TypeConstructor,
                types,
            ),
        )
    }

    /// [`Self::to_tagged`] built directly resident in `scope`'s own region and sealed as a
    /// delivered carrier — the shape a caller with no fold already in hand needs: the payload's
    /// `Record` substrate can only be born through a fold door, so this drives a zero-dep one over
    /// `scope`'s frame. The seed operand is a bare handle into that same region,
    /// so [`Delivered::restamp_in_place`](crate::witnessed::Delivered::restamp_in_place) builds the
    /// value where it already belongs and mints its description there: the region is the value's
    /// host *and* one of its members, since the freshly born substrate borrows into it. A consumer
    /// adopting this envelope under a copying seam therefore correctly retains `scope`'s frame.
    pub fn to_tagged_delivered<'a>(
        &self,
        scope: &'a Scope<'a>,
        registries: &RunRegistries,
    ) -> DeliveredCarried {
        let frame = scope_frame(scope);
        // The seed is a bare region handle living in this scope's own region — it borrows nothing,
        // so it seals resident under an empty foreign bundle.
        let seed = scope
            .deliver_resident::<RegionHandleFamily<KoanStorageProfile>>(scope.brand().handle());
        seed.restamp_in_place::<CarriedFamily, KoanStorageProfile>(
            &frame,
            |_handle, _dest, placement| {
                let owned_cells = crate::machine::core::FrameCoverage::empty();
                let brand = FoldingBrand::in_fold_closure(placement);
                Carried::Object(brand.alloc_object_folded(
                    self.to_tagged(brand.with_holder(&owned_cells), registries),
                ))
            },
        )
    }
}

/// A fixed literal of the error shape as the Type token it is, interned so a rendered member name
/// resolves back. Every caller passes a `KErrorKind` variant name or `"KError"`, all Type tokens by
/// construction.
fn error_label(text: &str, registries: &RunRegistries) -> TypeSymbol {
    TypeSymbol::declared(text, &registries.labels).expect("a KError variant name is a Type token")
}

/// A synthetic singleton member for an unregistered carrier (the `KError` to-tagged payload's
/// `type_id` and the wrapping `Tagged`'s `identity`). Its one member carries an empty schema —
/// these carriers are read directly by the TRY branch walker, never dispatched on, so the schema
/// is never consulted.
fn synthetic_singleton(name: TypeSymbol, kind: KKind, types: &TypeRegistry) -> KType {
    let schema = match kind {
        KKind::NewType => RelativeSchema::NewType(KType::ANY),
        _ => RelativeSchema::TypeConstructor {
            schema: TypeMemberMap::default(),
            param_names: Vec::new(),
        },
    };
    RecursiveGroupWindow::seal_singleton(name, schema, None, types)
}

/// The `KError` carrier type — the `TypeConstructor`-kind member a `to_tagged` value reports its
/// family from. Used as the `Error` arm of `CATCH`'s declared
/// `:(Result {Ok = Any, Error = KError})` return (a documentary contract — `KError` is not a
/// registered prelude type, and the synthetic member is identity-throwaway, but `CATCH`'s return
/// is never validated against the runtime value).
pub(crate) fn kerror_ktype(registries: &RunRegistries) -> KType {
    synthetic_singleton(
        error_label("KError", registries),
        KKind::TypeConstructor,
        &registries.types,
    )
}

impl KErrorKind {
    /// `(name, fields)` for `KError::to_tagged`. `name` is the capitalized variant tag —
    /// a TRY arm catches it by name (`TypeMismatch -> …`) — and also the payload newtype's
    /// identity. Field order mirrors the variant's declaration order; `frames` is appended
    /// by the caller. Dispatcher-internal kinds flatten to `{ kind, message }` since
    /// they're only catchable via `_`.
    fn to_struct_fields<'a>(&self, brand: RegionBrand<'a>) -> (String, Vec<(String, KObject<'a>)>) {
        match self {
            KErrorKind::TypeMismatch { arg, expected, got } => (
                "TypeMismatch".to_string(),
                vec![
                    (
                        "arg".to_string(),
                        KObject::KString(brand.allocator().text(arg)),
                    ),
                    (
                        "expected".to_string(),
                        KObject::KString(brand.allocator().text(expected)),
                    ),
                    (
                        "got".to_string(),
                        KObject::KString(brand.allocator().text(got)),
                    ),
                ],
            ),
            KErrorKind::MissingArg(name) => (
                "MissingArg".to_string(),
                vec![(
                    "name".to_string(),
                    KObject::KString(brand.allocator().text(name)),
                )],
            ),
            KErrorKind::UnboundName(name) => (
                "UnboundName".to_string(),
                vec![(
                    "name".to_string(),
                    KObject::KString(brand.allocator().text(name)),
                )],
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
                    (
                        "expr".to_string(),
                        KObject::KString(brand.allocator().text(expr)),
                    ),
                    (
                        "candidates".to_string(),
                        KObject::Number(*candidates as f64),
                    ),
                ],
            ),
            KErrorKind::DispatchFailed { expr, reason } => (
                "DispatchFailed".to_string(),
                vec![
                    (
                        "expr".to_string(),
                        KObject::KString(brand.allocator().text(expr)),
                    ),
                    (
                        "reason".to_string(),
                        KObject::KString(brand.allocator().text(reason)),
                    ),
                ],
            ),
            KErrorKind::NestedBinder { expr, .. } => (
                "NestedBinder".to_string(),
                vec![(
                    "expr".to_string(),
                    KObject::KString(brand.allocator().text(expr)),
                )],
            ),
            KErrorKind::ShapeError(msg) => (
                "ShapeError".to_string(),
                vec![(
                    "message".to_string(),
                    KObject::KString(brand.allocator().text(msg)),
                )],
            ),
            KErrorKind::ParseError {
                message,
                span,
                file,
            } => {
                let mut fields: Vec<(String, KObject<'a>)> = Vec::with_capacity(6);
                fields.push((
                    "message".to_string(),
                    KObject::KString(brand.allocator().text(message)),
                ));
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
                    KObject::KString(brand.allocator().text(&path.unwrap_or_default())),
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
                vec![(
                    "message".to_string(),
                    KObject::KString(brand.allocator().text(msg)),
                )],
            ),
            KErrorKind::Rebind { .. }
            | KErrorKind::DuplicateDeclaration { .. }
            | KErrorKind::DuplicateOverload { .. }
            | KErrorKind::TypeClassBindingExpectsType { .. }
            | KErrorKind::SchedulerDeadlock { .. } => {
                let name = match self {
                    KErrorKind::Rebind { .. } => "Rebind",
                    KErrorKind::DuplicateDeclaration { .. } => "DuplicateDeclaration",
                    KErrorKind::DuplicateOverload { .. } => "DuplicateOverload",
                    KErrorKind::TypeClassBindingExpectsType { .. } => "TypeClassBindingExpectsType",
                    KErrorKind::SchedulerDeadlock { .. } => "SchedulerDeadlock",
                    _ => unreachable!(),
                };
                (
                    name.to_string(),
                    vec![
                        (
                            "kind".to_string(),
                            KObject::KString(brand.allocator().text(name)),
                        ),
                        (
                            "message".to_string(),
                            KObject::KString(brand.allocator().text(&format!("{self}"))),
                        ),
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
            KErrorKind::NestedBinder { expr, suggest_flat } => {
                write!(
                    f,
                    "binder declaration in an eagerly evaluated sub-expression `{expr}`; a binder \
                     must be a statement or a lazily-captured body"
                )?;
                if *suggest_flat {
                    write!(
                        f,
                        ". To bind a name and register the definition in one statement, write it \
                         flat: `LET <name> = FN <signature> -> <Return> = (<body>)`, or the `OP` / \
                         `UNARY OP` twins"
                    )?;
                }
                Ok(())
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
            KErrorKind::DuplicateDeclaration {
                name,
                first,
                second,
            } => write!(
                f,
                "name '{name}' is declared twice in this block, by statement {first} and \
                 statement {second}; a binding is bind-once",
            ),
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
