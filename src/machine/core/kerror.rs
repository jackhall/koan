use std::fmt;

use crate::machine::model::WorkingExpression;
use crate::machine::model::{Carried, CarriedFamily, KObject};
use crate::machine::model::{StaticName, TypeSymbol};
use crate::source::{self, FileId, SourceLoc, SourceRef, Span};
use crate::witnessed::RegionHandleFamily;

use super::{DeliveredCarried, FoldingBrand, RegionBrand, SubstrateDoor};
use super::{KoanStorageProfile, Scope, scope_frame};
use crate::machine::model::RunRegistries;
use crate::machine::model::close_inference::DynamicNameForm;
use crate::machine::model::labels::{BinderSymbol, ValueSymbol};

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
    ///
    /// `location` is where the offending expression sits. A dispatch summary names the *type* each
    /// argument slot matched on, never its spelling, so the location is how a reader gets the
    /// spelling back — and unlike a [`TraceFrame`], it is present even when nothing encloses the
    /// expression. Display-only, like [`NestedBinder`](KErrorKind::NestedBinder)'s `suggest_flat`.
    AmbiguousDispatch {
        expr: String,
        candidates: usize,
        location: Option<SourceLoc>,
    },
    /// `location` carries the offending expression's site; see
    /// [`AmbiguousDispatch`](KErrorKind::AmbiguousDispatch) for why a dispatch error owns one.
    DispatchFailed {
        expr: String,
        reason: String,
        location: Option<SourceLoc>,
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
    /// Distinct from `Rebind` — collision is per-signature within one bucket. `name` is the
    /// bucket's whole untyped key as a capture pattern (`(DOUBLE _)`), rendered on this arm from
    /// the seal the write already holds; `signature` is the standing entry's typed identity.
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
    /// A form that resolves names dynamically sits inside a `CLOSE (<block>)`, so the block's free
    /// identifiers — the capture list the form would otherwise derive — are not readable off the
    /// text. `CLOSE OVER` is the form that admits one: its captures are written out, so the block
    /// needs no inference at all.
    ///
    /// `location` is where the offending form sits; see
    /// [`DispatchFailed`](KErrorKind::DispatchFailed) for why an error about a form owns one.
    DynamicNamesUnderInferredClose {
        form: DynamicNameForm,
        location: Option<SourceLoc>,
    },
    /// Scheduler drained its work queues with nodes still parked on
    /// dependencies that can no longer fire (dependency cycle).
    SchedulerDeadlock {
        pending: usize,
        sample: String,
    },
}

/// One entry in an error's call-stack trace. `function` names the frame — a call site's source
/// text, or a fixed scheduler-internal tag; `expression` renders what ran there — an expression's
/// `summarize()` text, or a callable's by-name type. `location` is `Some` when the originating
/// expression had both `span` and `file` populated.
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
    pub fn from_expr(
        function: impl Into<String>,
        expr: &WorkingExpression<'_>,
        registries: &RunRegistries,
    ) -> TraceFrame {
        TraceFrame {
            function: function.into(),
            expression: expr.summarize(registries),
            location: location_from_expr(expr),
        }
    }
}

/// Resolve a source extent to the 1-based location its start sits at.
pub(crate) fn resolve_location(site: SourceRef) -> SourceLoc {
    source::with(site.file, |f| {
        let (line, col_utf16) = f.resolve(site.span.start);
        SourceLoc {
            path: f.path.clone(),
            line,
            col_utf16,
        }
    })
}

/// Where an expression sits in source, `None` for a node the scheduler synthesized.
pub(crate) fn location_from_expr(expr: &WorkingExpression<'_>) -> Option<SourceLoc> {
    expr.source_ref().map(resolve_location)
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

    /// Lower this error into the `KError` union member for its kind — one
    /// [`KObject::Wrapped`] whose `type_id` is `member` and whose payload is a record-repr
    /// `Wrapped` mirroring the variant's fields plus `frames :List<Str>`, so an arm's `it.field`
    /// ATTR reads through the payload's own `Wrapped` arm. One identity spells the kind, so a
    /// rendered error names it once.
    ///
    /// The member is the registered `KError` union's member of that kind's name — the same member
    /// a `TRY` or `MATCH … OVER KError` arm names, so selection is an identity compare with
    /// nothing to re-derive. `KError` is a registered prelude union and only `TRY` / `CATCH`
    /// finishes reach this door, so a missing registration is a programming error.
    ///
    /// `door` is the substrate door the payload's `Record` substrate is born through — a caller with
    /// no fold in hand mints a zero-dep one (see [`Self::to_wrapped_delivered`]). Every cell here is
    /// freshly built owned data, so the door needs no holder.
    pub fn to_wrapped<'a>(
        &self,
        door: SubstrateDoor<'a, '_>,
        scope: &Scope<'_>,
        registries: &RunRegistries,
    ) -> KObject<'a> {
        let types = &registries.types;
        let brand = **door;
        let (kind_name, fields) = self.kind.to_struct_fields(brand);
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
        // Every label here is a name the error shape fixes in Rust source, classified where it is
        // written; recording it hands the run's label table the spelling a rendering resolves.
        let mut pairs: Vec<(&'static StaticName<ValueSymbol>, KObject<'a>)> = fields;
        pairs.push((&FIELD.frames, frames_list));
        let classified: Vec<(BinderSymbol, KObject<'a>)> = pairs
            .into_iter()
            .map(|(name, value)| (BinderSymbol::Value(registries.labels.record(name)), value))
            .collect();
        let record = KObject::record(door, &classified, types);
        let union = scope
            .resolve_type(KERROR.symbol())
            .expect("KError is registered in the prelude before any error can lower");
        let member = types
            .union_member_named(union, kind_name.symbol().symbol())
            .expect("every KErrorKind name is a member of the registered KError union");
        // `wrapped_hold`, not `wrapped_peel`: the record is the member's payload verbatim, and a
        // peel would collapse the one layer the kind's identity rides on.
        KObject::wrapped_hold(door, &record, member)
    }

    /// [`Self::to_wrapped`] built directly resident in `scope`'s own region and sealed as a
    /// delivered carrier — the shape a caller with no fold already in hand needs: the payload's
    /// `Record` substrate can only be born through a fold door, so this drives a zero-dep one over
    /// `scope`'s frame. The seed operand is a bare handle into that same region,
    /// so [`Delivered::restamp_in_place`](crate::witnessed::Delivered::restamp_in_place) builds the
    /// value where it already belongs and mints its description there: the region is the value's
    /// host *and* one of its members, since the freshly born substrate borrows into it. A consumer
    /// adopting this envelope under a copying seam therefore correctly retains `scope`'s frame.
    pub fn to_wrapped_delivered<'a>(
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
                Carried::Object(brand.alloc_object_folded(self.to_wrapped(
                    brand.with_holder(&owned_cells),
                    scope,
                    registries,
                )))
            },
        )
    }
}

/// The union name every lowered error reports its family under, and one name per `KErrorKind`
/// variant. Each is a Type token fixed in Rust source, so each is minted once for the process and
/// recorded into a run's interner when
/// [`error_union::register`](crate::builtins::error_union::register) mints the members.
///
/// The same table drives both ends: [`KErrorKind::to_struct_fields`] reads a lowered error's member name
/// off it, and the registration iterates [`ErrorKinds::all`] — so a variant's member and the name
/// its lowering looks up cannot drift apart.
pub(crate) struct ErrorKinds {
    pub ambiguous_dispatch: StaticName<TypeSymbol>,
    pub arity_mismatch: StaticName<TypeSymbol>,
    pub dispatch_failed: StaticName<TypeSymbol>,
    pub duplicate_declaration: StaticName<TypeSymbol>,
    pub duplicate_overload: StaticName<TypeSymbol>,
    pub dynamic_names_under_inferred_close: StaticName<TypeSymbol>,
    pub missing_arg: StaticName<TypeSymbol>,
    pub nested_binder: StaticName<TypeSymbol>,
    pub parse_error: StaticName<TypeSymbol>,
    pub rebind: StaticName<TypeSymbol>,
    pub scheduler_deadlock: StaticName<TypeSymbol>,
    pub shape_error: StaticName<TypeSymbol>,
    pub type_class_binding_expects_type: StaticName<TypeSymbol>,
    pub type_mismatch: StaticName<TypeSymbol>,
    pub unbound_name: StaticName<TypeSymbol>,
    pub user: StaticName<TypeSymbol>,
}

pub(crate) static KERROR: StaticName<TypeSymbol> = crate::static_name!(TypeSymbol, "KError");

pub(crate) static KIND: ErrorKinds = ErrorKinds {
    ambiguous_dispatch: crate::static_name!(TypeSymbol, "AmbiguousDispatch"),
    arity_mismatch: crate::static_name!(TypeSymbol, "ArityMismatch"),
    dispatch_failed: crate::static_name!(TypeSymbol, "DispatchFailed"),
    duplicate_declaration: crate::static_name!(TypeSymbol, "DuplicateDeclaration"),
    duplicate_overload: crate::static_name!(TypeSymbol, "DuplicateOverload"),
    dynamic_names_under_inferred_close: crate::static_name!(
        TypeSymbol,
        "DynamicNamesUnderInferredClose"
    ),
    missing_arg: crate::static_name!(TypeSymbol, "MissingArg"),
    nested_binder: crate::static_name!(TypeSymbol, "NestedBinder"),
    parse_error: crate::static_name!(TypeSymbol, "ParseError"),
    rebind: crate::static_name!(TypeSymbol, "Rebind"),
    scheduler_deadlock: crate::static_name!(TypeSymbol, "SchedulerDeadlock"),
    shape_error: crate::static_name!(TypeSymbol, "ShapeError"),
    type_class_binding_expects_type: crate::static_name!(TypeSymbol, "TypeClassBindingExpectsType"),
    type_mismatch: crate::static_name!(TypeSymbol, "TypeMismatch"),
    unbound_name: crate::static_name!(TypeSymbol, "UnboundName"),
    user: crate::static_name!(TypeSymbol, "User"),
};

impl ErrorKinds {
    /// Every kind name, in the order the `KError` union declares its members.
    pub(crate) fn all(&'static self) -> [&'static StaticName<TypeSymbol>; 16] {
        [
            &self.ambiguous_dispatch,
            &self.arity_mismatch,
            &self.dispatch_failed,
            &self.duplicate_declaration,
            &self.duplicate_overload,
            &self.dynamic_names_under_inferred_close,
            &self.missing_arg,
            &self.nested_binder,
            &self.parse_error,
            &self.rebind,
            &self.scheduler_deadlock,
            &self.shape_error,
            &self.type_class_binding_expects_type,
            &self.type_mismatch,
            &self.unbound_name,
            &self.user,
        ]
    }
}

/// The error record's field names, each fixed in Rust source rather than written by a program. A
/// caught error is an ordinary koan record — a handler reads `message` or `expr` off it by name —
/// so each spelling classifies once, at its first read, and records its text where a rendering
/// resolves it.
struct ErrorFields {
    arg: StaticName<ValueSymbol>,
    candidates: StaticName<ValueSymbol>,
    col_utf16: StaticName<ValueSymbol>,
    expected: StaticName<ValueSymbol>,
    expr: StaticName<ValueSymbol>,
    form: StaticName<ValueSymbol>,
    frames: StaticName<ValueSymbol>,
    got: StaticName<ValueSymbol>,
    kind: StaticName<ValueSymbol>,
    line: StaticName<ValueSymbol>,
    message: StaticName<ValueSymbol>,
    name: StaticName<ValueSymbol>,
    path: StaticName<ValueSymbol>,
    reason: StaticName<ValueSymbol>,
    span_end: StaticName<ValueSymbol>,
    span_start: StaticName<ValueSymbol>,
}

static FIELD: ErrorFields = ErrorFields {
    arg: crate::static_name!(ValueSymbol, "arg"),
    candidates: crate::static_name!(ValueSymbol, "candidates"),
    col_utf16: crate::static_name!(ValueSymbol, "col_utf16"),
    expected: crate::static_name!(ValueSymbol, "expected"),
    expr: crate::static_name!(ValueSymbol, "expr"),
    form: crate::static_name!(ValueSymbol, "form"),
    frames: crate::static_name!(ValueSymbol, "frames"),
    got: crate::static_name!(ValueSymbol, "got"),
    kind: crate::static_name!(ValueSymbol, "kind"),
    line: crate::static_name!(ValueSymbol, "line"),
    message: crate::static_name!(ValueSymbol, "message"),
    name: crate::static_name!(ValueSymbol, "name"),
    path: crate::static_name!(ValueSymbol, "path"),
    reason: crate::static_name!(ValueSymbol, "reason"),
    span_end: crate::static_name!(ValueSymbol, "span_end"),
    span_start: crate::static_name!(ValueSymbol, "span_start"),
};

impl KErrorKind {
    /// `(name, fields)` for [`KError::to_wrapped`]. `name` is the [`KIND`] entry for this
    /// variant — the `KError` union member the lowered value carries, which a TRY or
    /// `MATCH … OVER KError` arm catches by name (`TypeMismatch -> …`). Field order mirrors the
    /// variant's declaration order; `frames` is appended by the caller. Dispatcher-internal kinds
    /// flatten to `{ kind, message }`.
    fn to_struct_fields<'a>(
        &self,
        brand: RegionBrand<'a>,
    ) -> (
        &'static StaticName<TypeSymbol>,
        Vec<(&'static StaticName<ValueSymbol>, KObject<'a>)>,
    ) {
        match self {
            KErrorKind::TypeMismatch { arg, expected, got } => (
                &KIND.type_mismatch,
                vec![
                    (&FIELD.arg, KObject::KString(brand.allocator().text(arg))),
                    (
                        &FIELD.expected,
                        KObject::KString(brand.allocator().text(expected)),
                    ),
                    (&FIELD.got, KObject::KString(brand.allocator().text(got))),
                ],
            ),
            KErrorKind::MissingArg(name) => (
                &KIND.missing_arg,
                vec![(&FIELD.name, KObject::KString(brand.allocator().text(name)))],
            ),
            KErrorKind::UnboundName(name) => (
                &KIND.unbound_name,
                vec![(&FIELD.name, KObject::KString(brand.allocator().text(name)))],
            ),
            KErrorKind::ArityMismatch { expected, got } => (
                &KIND.arity_mismatch,
                vec![
                    (&FIELD.expected, KObject::Number(*expected as f64)),
                    (&FIELD.got, KObject::Number(*got as f64)),
                ],
            ),
            KErrorKind::AmbiguousDispatch {
                expr, candidates, ..
            } => (
                &KIND.ambiguous_dispatch,
                vec![
                    (&FIELD.expr, KObject::KString(brand.allocator().text(expr))),
                    (&FIELD.candidates, KObject::Number(*candidates as f64)),
                ],
            ),
            KErrorKind::DispatchFailed { expr, reason, .. } => (
                &KIND.dispatch_failed,
                vec![
                    (&FIELD.expr, KObject::KString(brand.allocator().text(expr))),
                    (
                        &FIELD.reason,
                        KObject::KString(brand.allocator().text(reason)),
                    ),
                ],
            ),
            KErrorKind::NestedBinder { expr, .. } => (
                &KIND.nested_binder,
                vec![(&FIELD.expr, KObject::KString(brand.allocator().text(expr)))],
            ),
            KErrorKind::DynamicNamesUnderInferredClose { form, .. } => (
                &KIND.dynamic_names_under_inferred_close,
                vec![(
                    &FIELD.form,
                    KObject::KString(brand.allocator().text(form.surface())),
                )],
            ),
            KErrorKind::ShapeError(msg) => (
                &KIND.shape_error,
                vec![(
                    &FIELD.message,
                    KObject::KString(brand.allocator().text(msg)),
                )],
            ),
            KErrorKind::ParseError {
                message,
                span,
                file,
            } => {
                let mut fields: Vec<(&'static StaticName<ValueSymbol>, KObject<'a>)> =
                    Vec::with_capacity(6);
                fields.push((
                    &FIELD.message,
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
                    &FIELD.span_start,
                    KObject::Number(span_start.unwrap_or(0) as f64),
                ));
                fields.push((
                    &FIELD.span_end,
                    KObject::Number(span_end.unwrap_or(0) as f64),
                ));
                fields.push((
                    &FIELD.path,
                    KObject::KString(brand.allocator().text(&path.unwrap_or_default())),
                ));
                fields.push((&FIELD.line, KObject::Number(line.unwrap_or(0) as f64)));
                fields.push((
                    &FIELD.col_utf16,
                    KObject::Number(col_utf16.unwrap_or(0) as f64),
                ));
                (&KIND.parse_error, fields)
            }
            KErrorKind::User(msg) => (
                &KIND.user,
                vec![(
                    &FIELD.message,
                    KObject::KString(brand.allocator().text(msg)),
                )],
            ),
            KErrorKind::Rebind { .. }
            | KErrorKind::DuplicateDeclaration { .. }
            | KErrorKind::DuplicateOverload { .. }
            | KErrorKind::TypeClassBindingExpectsType { .. }
            | KErrorKind::SchedulerDeadlock { .. } => {
                let name = match self {
                    KErrorKind::Rebind { .. } => &KIND.rebind,
                    KErrorKind::DuplicateDeclaration { .. } => &KIND.duplicate_declaration,
                    KErrorKind::DuplicateOverload { .. } => &KIND.duplicate_overload,
                    KErrorKind::TypeClassBindingExpectsType { .. } => {
                        &KIND.type_class_binding_expects_type
                    }
                    KErrorKind::SchedulerDeadlock { .. } => &KIND.scheduler_deadlock,
                    _ => unreachable!(),
                };
                (
                    name,
                    vec![
                        (
                            &FIELD.kind,
                            KObject::KString(brand.allocator().text(name.text())),
                        ),
                        (
                            &FIELD.message,
                            KObject::KString(brand.allocator().text(&format!("{self}"))),
                        ),
                    ],
                )
            }
        }
    }
}

/// A resolved site in the `at <path>:<line>:<col>` shape a trace frame uses, so a location reads
/// the same wherever it appears.
fn write_location(f: &mut fmt::Formatter<'_>, location: Option<&SourceLoc>) -> fmt::Result {
    let Some(loc) = location else { return Ok(()) };
    write!(f, " at {}:{}:{}", loc.path, loc.line, loc.col_utf16)
}

impl fmt::Display for KError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        for frame in &self.frames {
            write!(f, "\n  in {} ({})", frame.expression, frame.function)?;
            write_location(f, frame.location.as_ref())?;
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
            KErrorKind::AmbiguousDispatch {
                expr,
                candidates,
                location,
            } => {
                write!(
                    f,
                    "ambiguous dispatch: {candidates} candidates match {expr}"
                )?;
                write_location(f, location.as_ref())?;
                f.write_str(" with equal specificity")
            }
            KErrorKind::DispatchFailed {
                expr,
                reason,
                location,
            } => {
                write!(f, "dispatch failed for {expr}")?;
                write_location(f, location.as_ref())?;
                write!(f, ": {reason}")
            }
            KErrorKind::DynamicNamesUnderInferredClose { form, location } => {
                write!(f, "CLOSE: `{}`", form.surface())?;
                write_location(f, location.as_ref())?;
                write!(
                    f,
                    " {}, so the block's capture list cannot be inferred — name the captures with \
                     `CLOSE OVER (<names>) (<block>)`",
                    form.reason()
                )
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
