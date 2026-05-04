use std::collections::HashMap;
use std::rc::Rc;

// (CallArena is reference-counted via std::rc::Rc — re-exporting here for `BodyResult::Tail`
// users.)

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral, TypeExpr, TypeParams};

use super::arena::CallArena;
use super::kerror::{KError, KErrorKind};
use super::kobject::KObject;
use super::ktraits::Parseable;
use super::scope::{KFuture, Scope};

/// One position in a function's structural shape: a `Keyword` (fixed token) or a typeless
/// `Slot`. A sequence of these is the dispatch bucket key; overloads sharing a shape compete
/// on `KType` specificity within the bucket.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum UntypedElement {
    Keyword(String),
    Slot,
}

/// Bucket key produced by `ExpressionSignature::untyped_key` and `KExpression::untyped_key`.
/// They MUST agree on the same key for any signature/expression that should match. The parser
/// classifies source tokens into `ExpressionPart::Keyword` vs `ExpressionPart::Identifier` up
/// front using `is_keyword_token`; signatures map every `SignatureElement::Token` to
/// `Keyword`. `ExpressionSignature::normalize` uppercases lowercase registered tokens so the
/// two sides agree on the spelling.
pub type UntypedKey = Vec<UntypedElement>;

/// True iff `s` is a keyword (fixed token) rather than an identifier when classifying a source
/// token: no lowercase ASCII letters. `LET`, `=`, `THEN` qualify; `x`, `foo`, `Foo` don't.
/// Used by the parser's `classify_atom` and by `ExpressionSignature::normalize` to keep the
/// two ends of the dispatch contract aligned.
pub fn is_keyword_token(s: &str) -> bool {
    !s.chars().any(|c| c.is_ascii_lowercase())
}

/// Result of comparing two signatures' specificity. Returned by
/// `ExpressionSignature::specificity_vs`. `Equal` means "identical slot types"; `Incomparable`
/// means "neither dominates" — e.g. `<Number> <Any>` vs `<Any> <Number>` for an input that
/// matches both.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Specificity {
    StrictlyMore,
    StrictlyLess,
    Equal,
    Incomparable,
}

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
/// `Scheduler` impls this trait in `execute/scheduler.rs`. The single method is intentional —
/// a builtin's lever into the scheduler is "schedule this expression for late dispatch in the
/// given scope, and give me back a NodeId to forward my result through"; nothing else. The
/// scope is passed explicitly rather than read from a thread-local, so callers can spawn work
/// in any scope they hold (typically their own).
pub trait SchedulerHandle<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId;
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
/// replace with `frame = Some` drops the slot's previous frame immediately because lexical
/// scoping means the new frame's child scope's `outer` is the FN's captured scope, not the
/// previous frame's. `Err(KError)` propagates a structured failure; the scheduler short-
/// circuits any node whose dependency errored, appending a `Frame` as the error walks up.
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
}

impl<'a> KFunction<'a> {
    /// Construct a `KFunction`. `captured` is the FN's defining scope (or, for builtins,
    /// run-root — the scope they're being registered into).
    pub fn new(
        mut signature: ExpressionSignature,
        body: Body<'a>,
        captured: &'a Scope<'a>,
    ) -> Self {
        signature.normalize();
        let captured = captured as *const Scope<'_> as *const Scope<'static>;
        Self { signature, body, captured }
    }

    /// Re-attach the captured scope pointer to a fresh `'a` lifetime. The lifetime tracks
    /// the original scope's allocation, which by the SAFETY argument on the struct still
    /// lives.
    pub fn captured_scope(&self) -> &'a Scope<'a> {
        unsafe { std::mem::transmute::<*const Scope<'static>, &'a Scope<'a>>(self.captured) }
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
                            expected: arg.ktype.name().to_string(),
                            got: part.summarize(),
                        }));
                    }
                    args.insert(arg.name.clone(), Rc::new(part.resolve()));
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
        let pairs = match super::named_pairs::parse_named_value_pairs(&tmp_expr, "function call") {
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

/// The shape a function expects: an ordered mix of fixed `Token`s and typed `Argument` slots.
/// `Scope::dispatch` walks each registered function's signature looking for one whose
/// `matches` returns true for an incoming `KExpression`.
pub struct ExpressionSignature {
    pub return_type: KType,
    pub elements: Vec<SignatureElement>,
}

impl ExpressionSignature {
    pub fn matches(&self, expr: &KExpression<'_>) -> bool {
        if self.elements.len() != expr.parts.len() {
            return false;
        }
        self.elements.iter().zip(&expr.parts).all(|(el, part)| match (el, part) {
            (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) => s == t,
            (SignatureElement::Keyword(_), _) => false,
            (SignatureElement::Argument(arg), part) => arg.matches(part),
        })
    }

    /// Bucket key for this signature: keyword tokens become `Keyword(s)`, argument slots become
    /// `Slot`. Slot types are erased — same shape with different types lives in the same bucket
    /// and competes on specificity at dispatch time.
    pub fn untyped_key(&self) -> UntypedKey {
        self.elements
            .iter()
            .map(|el| match el {
                SignatureElement::Keyword(s) => UntypedElement::Keyword(s.clone()),
                SignatureElement::Argument(_) => UntypedElement::Slot,
            })
            .collect()
    }

    /// Registration-time fixup: uppercase any lowercase fixed `Token` so its bucket key matches
    /// what dispatch will compute from incoming expressions. TODO(monadic-effects): once
    /// effects exist, emit a warning here instead of silently rewriting — rejecting would lose
    /// the "drop in a builtin without thinking about caps" affordance.
    pub fn normalize(&mut self) {
        for el in &mut self.elements {
            if let SignatureElement::Keyword(s) = el {
                if s.chars().any(|c| c.is_ascii_lowercase()) {
                    *s = s.to_ascii_uppercase();
                }
            }
        }
    }

    /// Partial-order specificity comparison for overload tiebreaking. Assumes `self` and
    /// `other` share an `UntypedKey` (caller's responsibility) — only argument slots
    /// contribute, since fixed-token positions are equal by construction.
    pub fn specificity_vs(&self, other: &ExpressionSignature) -> Specificity {
        let mut any_more = false;
        let mut any_less = false;
        for (a, b) in self.elements.iter().zip(other.elements.iter()) {
            if let (SignatureElement::Argument(aa), SignatureElement::Argument(bb)) = (a, b) {
                if aa.ktype.is_more_specific_than(bb.ktype) {
                    any_more = true;
                } else if bb.ktype.is_more_specific_than(aa.ktype) {
                    any_less = true;
                }
            }
        }
        match (any_more, any_less) {
            (true, false) => Specificity::StrictlyMore,
            (false, true) => Specificity::StrictlyLess,
            (false, false) => Specificity::Equal,
            (true, true) => Specificity::Incomparable,
        }
    }
}

/// One slot in an `ExpressionSignature`: a literal `Token` that must match by string equality,
/// or a typed `Argument` whose value is captured into the `ArgumentBundle`.
pub enum SignatureElement {
    Keyword(String),
    Argument(Argument),
}

/// A typed parameter slot in a signature. `name` keys it in the `ArgumentBundle`; `ktype` gates
/// what `ExpressionPart`s it accepts.
pub struct Argument {
    pub name: String,
    pub ktype: KType,
}

impl Argument {
    /// Per-part type check.
    pub fn matches(&self, part: &ExpressionPart<'_>) -> bool {
        match self.ktype {
            KType::Any => true,
            KType::Number => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Number(_))
                    | ExpressionPart::Future(KObject::Number(_))
            ),
            KType::Str => matches!(
                part,
                ExpressionPart::Literal(KLiteral::String(_))
                    | ExpressionPart::Future(KObject::KString(_))
            ),
            KType::Bool => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Boolean(_))
                    | ExpressionPart::Future(KObject::Bool(_))
            ),
            KType::Null => matches!(
                part,
                ExpressionPart::Literal(KLiteral::Null) | ExpressionPart::Future(KObject::Null)
            ),
            KType::List => matches!(
                part,
                ExpressionPart::ListLiteral(_) | ExpressionPart::Future(KObject::List(_))
            ),
            KType::Dict => matches!(
                part,
                ExpressionPart::DictLiteral(_) | ExpressionPart::Future(KObject::Dict(_))
            ),
            KType::KFunction => matches!(
                part,
                ExpressionPart::Future(KObject::KFunction(_, _))
                    | ExpressionPart::Future(KObject::KFuture(_, _))
            ),
            KType::Identifier => matches!(part, ExpressionPart::Identifier(_)),
            KType::KExpression => matches!(part, ExpressionPart::Expression(_)),
            KType::TypeRef => matches!(part, ExpressionPart::Type(_)),
            KType::Type => matches!(
                part,
                ExpressionPart::Future(KObject::TaggedUnionType(_))
                    | ExpressionPart::Future(KObject::StructType { .. })
            ),
            KType::Tagged => matches!(
                part,
                ExpressionPart::Future(KObject::Tagged { .. })
            ),
            KType::Struct => matches!(
                part,
                ExpressionPart::Future(KObject::Struct { .. })
            ),
        }
    }
}

/// Type tags used by `Argument::matches` at dispatch time, by user-facing return-type
/// annotations on functions, and by the scheduler's runtime return-type check.
///
/// `KExpression` is the lazy slot: it accepts an unevaluated `ExpressionPart::Expression`
/// so the receiving builtin can choose when (or whether) to run it. `TypeRef` is a meta-type
/// for argument slots that capture a parsed type-name token (`ExpressionPart::Type(_)`) —
/// used by `FN`'s return-type annotation slot, not declarable in user code.
///
/// `Type` is the meta-type for any first-class type-value: a tagged-union schema produced by
/// `UNION` or a struct schema produced by `STRUCT` are both `KType::Type` at runtime, so
/// builtins that consume "a type" (construction primitives, future trait checks) can declare
/// a single slot and accept either form.
///
/// Future work: let users define duck types instead of an enum.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum KType {
    Number,
    Str,
    Bool,
    Null,
    List,
    Dict,
    KFunction,
    Identifier,
    KExpression,
    TypeRef,
    /// Meta-type for first-class type-values: `KObject::TaggedUnionType` and
    /// `KObject::StructType` both report this. Consumed by construction primitives and any
    /// builtin that takes "a type" as an argument.
    Type,
    /// A tagged value — one variant of a tagged union, carrying its tag and inner payload.
    /// Produced by `TAG`, consumed by `MATCH` to branch by tag.
    Tagged,
    /// A struct value — a record of named fields produced by a struct-type constructor.
    Struct,
    Any,
}

impl KType {
    /// Specificity ordering for `specificity_vs`. Concrete types outrank `Any`; concrete-vs-
    /// concrete is incomparable (mutually exclusive — a `Number` slot won't match a `Str`
    /// literal anyway). Returns `false` for equal types — strict, not reflexive.
    pub fn is_more_specific_than(self, other: KType) -> bool {
        !matches!(self, KType::Any) && matches!(other, KType::Any)
    }

    /// Short human-readable name for this type — used by error formatters and parsed back by
    /// `from_name` for surface-level annotations.
    pub fn name(self) -> &'static str {
        match self {
            KType::Number => "Number",
            KType::Str => "Str",
            KType::Bool => "Bool",
            KType::Null => "Null",
            KType::List => "List",
            KType::Dict => "Dict",
            KType::KFunction => "KFunction",
            KType::Identifier => "Identifier",
            KType::KExpression => "KExpression",
            KType::TypeRef => "TypeRef",
            KType::Type => "Type",
            KType::Tagged => "Tagged",
            KType::Struct => "Struct",
            KType::Any => "Any",
        }
    }

    /// Look up a `KType` by the textual name a user can write in source (e.g. `Number`,
    /// `KFunction`). Returns `None` for unknown names. `Identifier` and `TypeRef` are
    /// dispatch-time meta-types — not surface-declarable, since no `KObject` value carries
    /// them, so a function declaring such a return type could never satisfy its contract.
    pub fn from_name(name: &str) -> Option<KType> {
        match name {
            "Number" => Some(KType::Number),
            "Str" => Some(KType::Str),
            "Bool" => Some(KType::Bool),
            "Null" => Some(KType::Null),
            "List" => Some(KType::List),
            "Dict" => Some(KType::Dict),
            "KFunction" => Some(KType::KFunction),
            "KExpression" => Some(KType::KExpression),
            "Type" => Some(KType::Type),
            "Tagged" => Some(KType::Tagged),
            "Struct" => Some(KType::Struct),
            "Any" => Some(KType::Any),
            _ => None,
        }
    }

    /// Convert a parser `TypeExpr` into a `KType`. This is the surface-level type-parsing
    /// boundary used by FN signatures, FN return-type slots, and UNION/STRUCT field types.
    /// Phase 1 of container type parameterization handles only leaves — anything with
    /// non-empty `TypeParams` surfaces a deferred-feature error rather than corrupting the
    /// type tag. Phase 2 will replace this stub with structured KType variants.
    pub fn from_type_expr(t: &TypeExpr) -> Result<KType, String> {
        match &t.params {
            TypeParams::None => KType::from_name(&t.name)
                .ok_or_else(|| format!("unknown type name `{}`", t.name)),
            TypeParams::List(_) | TypeParams::Function { .. } => Err(format!(
                "parameterized type `{}` is not yet supported at the KType layer (phase 1: parser only)",
                t.render()
            )),
        }
    }

    /// True iff a runtime `KObject` value satisfies this declared type. `Any` matches
    /// everything; otherwise compare against the value's `ktype()`. Used by the scheduler's
    /// post-call return-type check.
    pub fn matches_value(self, obj: &KObject<'_>) -> bool {
        matches!(self, KType::Any) || self == obj.ktype()
    }
}


