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
    KExpression(KExpression),
    KTask(KFuture<'a>),
    KFunction(&'a KFunction<'a>),
    Null,
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
            KObject::KTask(t) => t.parsed.summarize(),
            KObject::KFunction(f) => f.summarize(),
            KObject::Null => "null".to_string(),
        }
    }
}
