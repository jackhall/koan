use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

use super::kobject::KObject;
use super::scope::{KFuture, Scope};


/// A callable Koan function: its `ExpressionSignature` (the call shape it matches) plus a
/// reference to the `Scope` where it was defined. `Scope::dispatch` finds the right `KFunction`
/// by signature and then `bind`s a `KExpression` into a `KFuture`.
pub struct KFunction<'a> {
    pub scope: &'a Scope<'a>,
    pub signature: ExpressionSignature,
}

impl<'a> KFunction<'a> {
    pub fn new(scope: &'a Scope<'a>, signature: ExpressionSignature) -> Self {
        Self { scope, signature }
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

    pub fn bind(&'a self, expr: KExpression) -> Result<KFuture<'a>, String> {
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
    pub elements: Vec<SignatureElement>,
}

impl ExpressionSignature {
    pub fn matches(&self, expr: &KExpression) -> bool {
        if self.elements.len() != expr.parts.len() {
            return false;
        }
        self.elements.iter().zip(&expr.parts).all(|(el, part)| match (el, part) {
            (SignatureElement::Token(s), ExpressionPart::Token(t)) => s == t,
            (SignatureElement::Token(_), _) => false,
            (SignatureElement::Argument(arg), part) => arg.matches(part),
        })
    }
}

/// One slot in an `ExpressionSignature`: a literal `Token` that must match by string equality,
/// or a typed `Argument` whose value is captured into the `ArgumentBundle`.
pub enum SignatureElement {
    Token(String),
    Argument(Argument),
}

/// A typed parameter slot in a signature. `name` keys it in the `ArgumentBundle`; `ktype` gates
/// what `ExpressionPart`s it accepts; `variadic` is reserved for future varargs support.
pub struct Argument {
    pub name: String,
    pub ktype: KType,
    pub variadic: bool,
}

impl Argument {
    pub fn matches(&self, part: &ExpressionPart) -> bool {
        match self.ktype {
            KType::Any => true,
            KType::Number => matches!(part, ExpressionPart::Literal(KLiteral::Number(_))),
            KType::Str => matches!(part, ExpressionPart::Literal(KLiteral::String(_))),
            KType::Bool => matches!(part, ExpressionPart::Literal(KLiteral::Boolean(_))),
            KType::Null => matches!(part, ExpressionPart::Literal(KLiteral::Null)),
        }
    }
}

/// Built-in type tags used by `Argument::matches` to reject ill-typed call sites at dispatch
/// time. In the future this should not assume all types can be enumerated; the user should be
/// able to define duck types.
pub enum KType {
    Number,
    Str,
    Bool,
    Null,
    Any,
}

