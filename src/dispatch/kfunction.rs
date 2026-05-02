use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

use super::kobject::KObject;
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
/// a builtin's lever into the scheduler is "schedule this expression for late dispatch and give
/// me back a NodeId to forward my result through"; nothing else.
pub trait SchedulerHandle<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>) -> NodeId;
}

/// What a builtin's body returns. `Value` is the common case — the body computed its result
/// inline. `Tail(expr)` says "my result is whatever this expression produces, evaluate it in
/// place"; the scheduler rewrites the current node's work to a fresh `Dispatch(expr)` and
/// re-runs the same slot, so a chain of tail calls (or unbounded tail recursion) reuses one
/// slot rather than allocating a new one per step. Used by `if_then` for its lazy `value`
/// slot and by `KFunction::invoke` for `Body::UserDefined`.
pub enum BodyResult<'a> {
    Value(&'a KObject<'a>),
    Tail(KExpression<'a>),
}

/// A function pointer that implements a builtin `KFunction`'s body. `for<'a>` so a single `fn`
/// works for any caller scope lifetime; the `&mut dyn SchedulerHandle<'a>` is the lever a body
/// uses to defer sub-expression evaluation back to the scheduler.
pub type BuiltinFn = for<'a> fn(
    &mut Scope<'a>,
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

/// A callable Koan function: its `ExpressionSignature` (the call shape it matches), an optional
/// reference to the `Scope` where it was defined (`None` for builtins), and the body
/// implementation. `Scope::dispatch` finds the right `KFunction` by signature and then `bind`s a
/// `KExpression` into a `KFuture`; the body runs via `KFunction::invoke` at execute time.
pub struct KFunction<'a> {
    pub scope: Option<&'a Scope<'a>>,
    pub signature: ExpressionSignature,
    pub body: Body<'a>,
}

impl<'a> KFunction<'a> {
    pub fn new(scope: Option<&'a Scope<'a>>, mut signature: ExpressionSignature, body: Body<'a>) -> Self {
        signature.normalize();
        Self { scope, signature, body }
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

    pub fn bind(&'a self, expr: KExpression<'a>) -> Result<KFuture<'a>, String> {
        if self.signature.elements.len() != expr.parts.len() {
            return Err(format!(
                "expected {} parts, got {}",
                self.signature.elements.len(),
                expr.parts.len()
            ));
        }
        let mut args: HashMap<String, Rc<KObject<'a>>> = HashMap::new();
        for (el, part) in self.signature.elements.iter().zip(expr.parts.iter()) {
            match el {
                SignatureElement::Keyword(s) => match part {
                    ExpressionPart::Keyword(t) if s == t => {}
                    ExpressionPart::Keyword(t) => {
                        return Err(format!("expected keyword '{s}', got '{t}'"));
                    }
                    _ => return Err(format!("expected keyword '{s}'")),
                },
                SignatureElement::Argument(arg) => {
                    if !arg.matches(part) {
                        return Err(format!("type mismatch for argument '{}'", arg.name));
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
            KType::Identifier => matches!(part, ExpressionPart::Identifier(_)),
            KType::KExpression => matches!(part, ExpressionPart::Expression(_)),
        }
    }
}

/// Type tags used by `Argument::matches` at dispatch time. `KExpression` is the lazy slot:
/// it accepts an unevaluated `ExpressionPart::Expression` so the receiving builtin can choose
/// when (or whether) to run it. Future work: let users define duck types instead of an enum.
#[derive(Copy, Clone)]
pub enum KType {
    Number,
    Str,
    Bool,
    Null,
    Identifier,
    KExpression,
    Any,
}

impl KType {
    /// Specificity ordering for `specificity_vs`. Concrete types outrank `Any`; concrete-vs-
    /// concrete is incomparable (mutually exclusive — a `Number` slot won't match a `Str`
    /// literal anyway). Returns `false` for equal types — strict, not reflexive.
    pub fn is_more_specific_than(self, other: KType) -> bool {
        !matches!(self, KType::Any) && matches!(other, KType::Any)
    }
}


