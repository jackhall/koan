use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

use super::kobject::KObject;
use super::scope::{KFuture, Scope};

/// One position in a function's structural shape: a `Fixed` token or a typeless `Slot`. A
/// sequence of these is the dispatch bucket key; overloads sharing a shape compete on `KType`
/// specificity within the bucket.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum UntypedElement {
    Fixed(String),
    Slot,
}

/// Bucket key produced by `ExpressionSignature::untyped_key` and `KExpression::untyped_key`.
/// They MUST agree on the same key for any signature/expression that should match — the
/// "fixed tokens are uppercase, lowercase identifiers are slots" rule below is the contract.
pub type UntypedKey = Vec<UntypedElement>;

/// True iff `s` is a fixed token rather than an identifier slot when computing an
/// `UntypedKey`: no lowercase ASCII letters. `LET`, `=`, `THEN` qualify; `x`, `foo`, `Foo`
/// don't. Registration uppercases lowercase tokens so user source and registered signatures
/// agree on the key.
pub fn is_fixed_token(s: &str) -> bool {
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

/// A function pointer that implements a `KFunction`'s body: takes the call-site `Scope` and the
/// resolved `ArgumentBundle` and produces a `KObject`. `for<'a>` so a single `fn` can be invoked
/// against any caller scope lifetime.
pub type BuiltinFn = for<'a> fn(&mut Scope<'a>, ArgumentBundle<'a>) -> &'a KObject<'a>;

/// A callable Koan function: its `ExpressionSignature` (the call shape it matches), an optional
/// reference to the `Scope` where it was defined (`None` for builtins), and the function pointer
/// implementing its body. `Scope::dispatch` finds the right `KFunction` by signature and then
/// `bind`s a `KExpression` into a `KFuture`.
pub struct KFunction<'a> {
    pub scope: Option<&'a Scope<'a>>,
    pub signature: ExpressionSignature,
    pub body: BuiltinFn,
}

impl<'a> KFunction<'a> {
    pub fn new(scope: Option<&'a Scope<'a>>, mut signature: ExpressionSignature, body: BuiltinFn) -> Self {
        signature.normalize();
        Self { scope, signature, body }
    }

    pub fn summarize(&self) -> String {
        let parts: Vec<String> = self
            .signature
            .elements
            .iter()
            .map(|el| match el {
                SignatureElement::Token(s) => s.clone(),
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
                SignatureElement::Token(s) => match part {
                    ExpressionPart::Token(t) if s == t => {}
                    ExpressionPart::Token(t) => {
                        return Err(format!("expected token '{s}', got '{t}'"));
                    }
                    _ => return Err(format!("expected token '{s}'")),
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
            (SignatureElement::Token(s), ExpressionPart::Token(t)) => s == t,
            (SignatureElement::Token(_), _) => false,
            (SignatureElement::Argument(arg), part) => arg.matches(part),
        })
    }

    /// Bucket key for this signature: fixed tokens become `Fixed(s)`, argument slots become
    /// `Slot`. Slot types are erased — same shape with different types lives in the same bucket
    /// and competes on specificity at dispatch time.
    pub fn untyped_key(&self) -> UntypedKey {
        self.elements
            .iter()
            .map(|el| match el {
                SignatureElement::Token(s) => UntypedElement::Fixed(s.clone()),
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
            if let SignatureElement::Token(s) = el {
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
    Token(String),
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
            KType::Identifier => matches!(part, ExpressionPart::Token(_)),
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


