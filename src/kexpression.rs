use crate::kobject::KObject;
use crate::ktraits::{Parseable, Executable};
use std::collections::HashMap;

pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

pub enum ExpressionPart {
    Token(String),
    Expression(Box<KExpression>),
    Literal(KLiteral),
}

pub struct KExpression {
    pub base: KObject,
    pub parts: Vec<ExpressionPart>,
}

impl Parseable for KExpression {
    fn equal(&self, other: &dyn Parseable) -> bool { self.base.equal(other) }
    fn summarize(&self) -> String {
        self.parts.iter()
            .map(|p| match p {
                ExpressionPart::Token(s) => s.clone(),
                ExpressionPart::Expression(e) => e.summarize(),
                ExpressionPart::Literal(lit) => match lit {
                    KLiteral::Number(n) => n.to_string(),
                    KLiteral::String(s) => s.clone(),
                    KLiteral::Boolean(b) => b.to_string(),
                    KLiteral::Null => "null".to_string(),
                },
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Executable for KExpression {
    fn execute(&self, _args: &[&dyn Parseable]) -> Box<dyn Parseable> {
        Box::new(KObject { name: self.summarize(), remaining_args: HashMap::new() })
    }
}
