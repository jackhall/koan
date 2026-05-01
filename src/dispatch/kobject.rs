use std::collections::HashMap;

use crate::parse::kexpression::KExpression;
use super::ktraits::{Parseable, Serializable};
use super::kfunction::KFunction;
use super::scope::KFuture;

/// Runtime value: scalars, collections, an unevaluated expression, a bound-but-unrun task, or a
/// reference to a function in some scope. The universal value type that `KFunction`s consume
/// and produce; implements `Parseable` so values can be compared and rendered uniformly.
pub enum KObject<'a> {
    UserDefined,
    Number(f64),
    KString(String),
    Bool(bool),
    List(Vec<KObject<'a>>),
    Dict(HashMap<Box<dyn Serializable + 'a>, KObject<'a>>),
    KExpression(KExpression<'a>),
    KFuture(KFuture<'a>),
    KFunction(&'a KFunction<'a>),
    Null,
}

impl<'a> KObject<'a> {
    /// Recursive clone that preserves structure for compound variants (`List`, `KExpression`).
    /// `Dict` and `KFuture` are not deep-cloneable in general (the former carries `Box<dyn
    /// Serializable>` keys whose owners we can't duplicate; the latter carries an
    /// `ArgumentBundle` of `Rc`-shared values that aren't cloneable here), so they fall back to
    /// `Null`. Used by `ExpressionPart::resolve` when materializing a `Future`-borne value
    /// into a fresh `KObject`, and by the scheduler's `Aggregate` node when copying each
    /// list-literal element's result into the produced `KObject::List`.
    pub fn deep_clone(&self) -> KObject<'a> {
        match self {
            KObject::Number(n) => KObject::Number(*n),
            KObject::KString(s) => KObject::KString(s.clone()),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            KObject::UserDefined => KObject::UserDefined,
            KObject::List(items) => KObject::List(items.iter().map(|i| i.deep_clone()).collect()),
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFunction(f) => KObject::KFunction(*f),
            KObject::Dict(_) | KObject::KFuture(_) => KObject::Null,
        }
    }
}

impl<'a> Parseable for KObject<'a> {
    fn equal(&self, other: &dyn Parseable) -> bool {
        self.summarize() == other.summarize()
    }
    fn summarize(&self) -> String {
        match self {
            KObject::UserDefined => "null".to_string(),
            KObject::Number(n) => n.to_string(),
            KObject::KString(s) => s.clone(),
            KObject::Bool(b) => b.to_string(),
            KObject::List(items) => {
                let parts: Vec<String> = items.iter().map(|i| i.summarize()).collect();
                format!("[{}]", parts.join(", "))
            }
            KObject::Dict(entries) => {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::KExpression(e) => e.summarize(),
            KObject::KFuture(t) => t.parsed.summarize(),
            KObject::KFunction(f) => f.summarize(),
            KObject::Null => "null".to_string(),
        }
    }
}
